// Runs Pyodide and pyo3_bindgen off the page's main thread, so the page stays
// responsive during long generations. The page cancels a call by terminating
// this worker and starting a new one; Python cannot be interrupted otherwise
// without cross-origin isolation, which a static host cannot provide.

let pyodide;
let playground;

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
  pyodide = await loadPyodide({ indexURL: pyodideUrl });
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
  async generateSource({ source, module, options }) {
    await pyodide.loadPackagesFromImports(source, quiet);
    return toJs(withPy([options], (values) => playground.generate_source(source, module, values)));
  },
  crateZip({ files, name }) {
    return toJs(withPy([files], (contents) => playground.crate_zip(contents, name)));
  },
};

self.onmessage = async ({ data }) => {
  const { id, command, args } = data;
  const started = performance.now();
  try {
    const result = await commands[command](args ?? {});
    const ms = performance.now() - started;
    const transfer = result instanceof Uint8Array ? [result.buffer] : [];
    postMessage({ id, ok: true, result, ms }, transfer);
  } catch (error) {
    reply(id, { ok: false, error: errorText(error), ms: performance.now() - started });
  }
};
