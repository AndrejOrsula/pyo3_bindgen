// Runs Pyodide and pyo3_bindgen off the page's main thread, so the page stays
// responsive during long generations. The page cancels a call by terminating
// this worker and starting a new one; Python cannot be interrupted otherwise
// without cross-origin isolation, which a static host cannot provide.

let pyodide;
let playground;
let rustLoadAttempts = 0;
const loadedRustModules = new Set();
const MAX_RUST_LOAD_ATTEMPTS = 8;
const MAX_RUST_OUTPUT_CHARS = 64 * 1024;
const MAX_RUST_ERROR_CHARS = 2048;
const ordinaryStdout = (text) => console.log(text);
const ordinaryStderr = (text) => console.error(text);

const reply = (id, payload) => postMessage({ id, ...payload });
const progress = (step, text) => postMessage({ type: 'progress', step, text });
const quiet = { messageCallback: () => {} };

// The line of a Python traceback that names the error; the frames are noise here,
// and some messages, such as micropip's, continue with hints on further lines.
const ERROR_LINE = /^[A-Za-z_][\w.]*(Error|Exception)\b/;

function errorText(error) {
  const lines = String(error?.message ?? error).trim().split('\n');
  const line = lines.findLast((text) => ERROR_LINE.test(text)) ?? lines[lines.length - 1];
  return line.trim().replace(/^RuntimeError: (?=\w+(Error|Exception|Exceeded)\b)/, '');
}

function boundedRustErrorText(error) {
  const value = error?.message ?? error;
  const message = (typeof value === 'string' ? value : String(value)).slice(0, MAX_RUST_ERROR_CHARS);
  const lines = message.trim().split('\n');
  const line = lines.findLast((text) => ERROR_LINE.test(text)) ?? lines[lines.length - 1];
  return line.trim().replace(/^RuntimeError: (?=\w+(Error|Exception|Exceeded)\b)/, '')
    .slice(0, MAX_RUST_ERROR_CHARS);
}

function rustRuntimeNeedsReset(error) {
  const detail = `${error?.name ?? ''} ${error?.message ?? error}`.slice(0, MAX_RUST_ERROR_CHARS);
  return (typeof WebAssembly !== 'undefined' && error instanceof WebAssembly.RuntimeError)
    || /(?:\bunreachable\b|memory access out of bounds|table index is out of bounds|indirect call|ExitStatus|abort\(\)|longjmp|Pyodide already fatally failed)/i.test(detail);
}

const toJs = (value) => {
  const result = value.toJs({ dict_converter: Object.fromEntries, create_pyproxies: false });
  value.destroy();
  return result;
};

// Calls `action` with Python copies of `values`, then frees them.
function withPy(values, action) {
  const proxies = values.map((value) => pyodide.toPy(value));
  try {
    return action(...proxies);
  } finally {
    for (const proxy of proxies) proxy?.destroy?.();
  }
}

async function init({ pyodideUrl, wheelUrl, glueUrl, generatorRevision = '' }) {
  progress('python', 'Downloading Python…');
  const { loadPyodide } = await import(new URL('pyodide.mjs', pyodideUrl).href);
  pyodide = await loadPyodide({ indexURL: pyodideUrl, stdout: ordinaryStdout, stderr: ordinaryStderr });
  progress('generator', 'Installing pyo3_bindgen…');
  await pyodide.loadPackage('micropip', quiet);
  const micropip = pyodide.pyimport('micropip');
  await micropip.install(new URL(wheelUrl, self.location.href).href);
  micropip.destroy();
  const glue = await fetch(new URL(glueUrl, self.location.href));
  if (!glue.ok) throw new Error(`cannot load ${glueUrl}: HTTP ${glue.status}`);
  pyodide.FS.mkdirTree('/playground');
  pyodide.FS.writeFile('/playground/playground.py', await glue.text());
  pyodide.runPython("import sys; sys.path.insert(0, '/playground')");
  playground = pyodide.pyimport('playground');
  playground.configure(generatorRevision);
  return {
    python: pyodide.runPython("import platform; platform.python_version()"),
    pyodide: pyodide.version,
    defaults: toJs(playground.default_options()),
  };
}

const commands = {
  init,
  async install({ requirement }) {
    return toJs(await playground.install(requirement));
  },
  // Pyodide ships some packages, such as NumPy, that load on first import.
  async generateModules({ modules, options }) {
    await pyodide.loadPackagesFromImports(modules.map((name) => `import ${name}`).join('\n'), quiet);
    return toJs(withPy([modules, options], (names, values) => playground.generate_modules(names, values)));
  },
  async generateSource({ source, module, options, mainBody = null }) {
    await pyodide.loadPackagesFromImports(source, quiet);
    return toJs(withPy([options], (values) => mainBody === null
      ? playground.generate_source(source, module, values)
      : playground.generate_source(source, module, values, mainBody)));
  },
  crateZip({ files, name }) {
    return toJs(withPy([files], (contents) => playground.crate_zip(contents, name)));
  },
  runRust({ moduleName, wasm }, id) {
    if (!/^[A-Za-z_]\w*$/.test(moduleName ?? '')) {
      return { success: false, stdout: '', stderr: '', truncated: false, error: 'invalid Rust module name' };
    }
    if (!(wasm instanceof Uint8Array)) {
      return { success: false, stdout: '', stderr: '', truncated: false, error: 'Rust module bytes must be a Uint8Array' };
    }
    if (loadedRustModules.has(moduleName)) {
      return { success: false, stdout: '', stderr: '', truncated: false, error: 'Rust module name was already used' };
    }
    if (rustLoadAttempts >= MAX_RUST_LOAD_ATTEMPTS) {
      return {
        success: false, stdout: '', stderr: '', truncated: false,
        error: 'Rust module load limit reached; restart the worker', resetRequired: true,
      };
    }

    // The extension's PyInit symbol fixes its import name at compile time.
    // Count before touching the filesystem or invoking the loader.
    rustLoadAttempts += 1;
    loadedRustModules.add(moduleName);
    const path = `/rust-side-modules/${rustLoadAttempts}/${moduleName}.so`;
    const outputs = {
      stdout: { text: '', decoder: new TextDecoder('utf-8', { ignoreBOM: true }) },
      stderr: { text: '', decoder: new TextDecoder('utf-8', { ignoreBOM: true }) },
    };
    let truncated = false;
    const captureText = (stream, text) => {
      const output = outputs[stream];
      const remaining = MAX_RUST_OUTPUT_CHARS - output.text.length;
      let end = Math.min(text.length, Math.max(0, remaining));
      if (end < text.length && end > 0) {
        const last = text.charCodeAt(end - 1);
        if (last >= 0xd800 && last <= 0xdbff) end -= 1;
      }
      const kept = text.slice(0, end);
      output.text += kept;
      if (kept.length < text.length) truncated = true;
      if (kept.length > 0) postMessage({ type: 'run-output', id, stream, text: kept });
    };
    const capture = (stream) => ({
      write(buffer) {
        captureText(stream, outputs[stream].decoder.decode(buffer, { stream: true }));
        return buffer.byteLength;
      },
    });
    const flush = (stream) => captureText(stream, outputs[stream].decoder.decode());
    let failure;
    let succeeded = false;
    let resetRequired = false;
    try {
      pyodide.setStdout(capture('stdout'));
      pyodide.setStderr(capture('stderr'));
      pyodide.FS.mkdirTree(path.slice(0, path.lastIndexOf('/')));
      pyodide.FS.writeFile(path, wasm);
      const quotedName = JSON.stringify(moduleName);
      const quotedPath = JSON.stringify(path);
      pyodide.runPython(`
import importlib.util as _p3_util, sys as _p3_sys
_p3_old_stdout, _p3_old_stderr = _p3_sys.stdout, _p3_sys.stderr
try:
    _p3_spec = _p3_util.spec_from_file_location(${quotedName}, ${quotedPath})
    if _p3_spec is None or _p3_spec.loader is None:
        raise ImportError("cannot load Rust side module")
    _p3_module = _p3_util.module_from_spec(_p3_spec)
    _p3_sys.modules[${quotedName}] = _p3_module
    try:
        _p3_spec.loader.exec_module(_p3_module)
    except BaseException:
        _p3_sys.modules.pop(${quotedName}, None)
        raise
    _p3_module.run_main()
finally:
    _p3_sys.stdout, _p3_sys.stderr = _p3_old_stdout, _p3_old_stderr
`);
      succeeded = true;
    } catch (error) {
      failure = error;
    } finally {
      for (const stream of ['stdout', 'stderr']) {
        try {
          flush(stream);
        } catch (error) {
          failure ??= error;
          resetRequired = true;
        }
      }
      try {
        pyodide.setStdout({ batched: ordinaryStdout });
      } catch (error) {
        failure ??= error;
        resetRequired = true;
      }
      try {
        pyodide.setStderr({ batched: ordinaryStderr });
      } catch (error) {
        failure ??= error;
        resetRequired = true;
      }
    }

    if (succeeded && !resetRequired) {
      return {
        success: true,
        stdout: outputs.stdout.text,
        stderr: outputs.stderr.text,
        truncated,
      };
    }
    const error = boundedRustErrorText(failure);
    resetRequired ||= rustRuntimeNeedsReset(failure);
    return {
      success: false, stdout: outputs.stdout.text, stderr: outputs.stderr.text, truncated, error,
      ...(resetRequired ? { resetRequired: true } : {}),
    };
  },
};

self.onmessage = async ({ data }) => {
  const { id, command, args } = data;
  const started = performance.now();
  try {
    const result = command === 'runRust'
      ? commands[command](args ?? {}, id)
      : await commands[command](args ?? {});
    const ms = performance.now() - started;
    const transfer = result instanceof Uint8Array ? [result.buffer] : [];
    postMessage({ id, ok: true, result, ms }, transfer);
  } catch (error) {
    reply(id, { ok: false, error: errorText(error), ms: performance.now() - started });
  }
};
