const MAX_DIAGNOSTICS = 65_536;
const MAX_SOURCE_BYTES = 16 * 1024 * 1024;
const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024;
const MAX_ASSET_CHUNK_BYTES = 16 * 1024 * 1024;
const MAX_RUNTIME_TAR_BYTES = 512 * 1024 * 1024;
const EXPECTED = {
  schema_version: 1,
  rust_version: '1.93.0',
  pyo3_version: '0.29.2',
  pyodide_version: '314.0.7',
  target: 'wasm32-unknown-emscripten',
  memory_bytes: 1_073_741_824,
};

let job = null;
let nextModule = 1;
const modulePrefix = [...crypto.getRandomValues(new Uint8Array(8))]
  .map((byte) => byte.toString(16).padStart(2, '0')).join('');
let emulator = null;
let emulatorReady = false;
let serialTail = '';
let serialCapture = null;
let serialCaptureLength = 0;
let serialWaiters = [];
let manifestPromise = null;
let linkerPromise = null;
let wasiClassesPromise = null;
let v86ObjectUrl = null;

const send = (id, payload, transfer = []) => postMessage({ id, ...payload }, transfer);
const progress = (id, phase, text) => send(id, { type: 'progress', phase, text });
const clamp = (text) => String(text ?? '').slice(-MAX_DIAGNOSTICS);

function fail(id, message, diagnostics = '', fatal = false) {
  send(id, { type: 'error', message, diagnostics: clamp(diagnostics), fatal });
}

function safeRelativePath(path) {
  return typeof path === 'string' && path.length > 0 && path.length <= 255
    && !path.startsWith('/') && !path.includes('\\') && !path.includes('\0')
    && path.split('/').every((part) => part && part !== '.' && part !== '..');
}

function sameOriginUrl(value, base, label) {
  const url = new URL(value, base);
  if (url.origin !== base.origin || !['http:', 'https:'].includes(url.protocol)) {
    throw new Error(`${label} must use the manifest origin`);
  }
  return url;
}

async function fetchChecked(asset, base, label, { exactSize = true } = {}) {
  if (!asset || !safeRelativePath(asset.path) || !Number.isSafeInteger(asset.bytes) || asset.bytes < 1
      || asset.bytes > MAX_ASSET_CHUNK_BYTES || !/^[a-f0-9]{64}$/.test(asset.sha256)) {
    throw new Error(`Invalid ${label} asset descriptor`);
  }
  const response = await fetch(sameOriginUrl(asset.path, base, label));
  if (!response.ok) throw new Error(`${label}: HTTP ${response.status}`);
  const reader = response.body?.getReader();
  if (!reader) throw new Error(`${label} response has no readable body`);
  const bytes = new Uint8Array(asset.bytes);
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (length + value.byteLength > asset.bytes) {
        await reader.cancel();
        throw new Error(`${label} exceeds its declared size`);
      }
      bytes.set(value, length);
      length += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  if ((exactSize && length !== asset.bytes) || length > asset.bytes) {
    throw new Error(`${label} size mismatch`);
  }
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  const hash = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  if (hash !== asset.sha256) throw new Error(`${label} SHA-256 mismatch`);
  return bytes;
}

function validateManifest(value, base) {
  for (const [key, expected] of Object.entries(EXPECTED)) {
    if (value?.[key] !== expected) throw new Error(`Unsupported compiler manifest ${key}`);
  }
  if (!value.v86 || !value.runtime || !value.dependencies || !value.linker
      || typeof value.v86.module !== 'string' || typeof value.wasi_module !== 'string') {
    throw new Error('Compiler manifest is incomplete');
  }
  for (const key of ['module']) sameOriginUrl(value.v86[key], base, `v86 ${key}`);
  sameOriginUrl(value.wasi_module, base, 'WASI module');
  for (const key of ['wasm', 'kernel']) {
    if (!value.v86[key]) throw new Error(`Compiler manifest has no v86 ${key}`);
  }
  if (!Array.isArray(value.runtime.chunks) || !value.runtime.chunks.length
      || !Array.isArray(value.dependencies.chunks) || !value.dependencies.chunks.length
      || !Array.isArray(value.linker.chunks) || !value.linker.chunks.length
      || value.linker.encoding !== 'gzip' || !/^[a-f0-9]{64}$/.test(value.linker.sha256)
      || !Number.isSafeInteger(value.linker.bytes) || value.linker.bytes < 1
      || value.linker.bytes > 120 * 1024 * 1024) {
    throw new Error('Compiler manifest has invalid chunk metadata');
  }
  if (!/^libpyo3-[a-f0-9]+\.rlib$/.test(value.dependencies.pyo3_rlib ?? '')) {
    throw new Error('Compiler manifest has an invalid PyO3 archive name');
  }
  for (const asset of [...value.runtime.chunks, ...value.dependencies.chunks, ...value.linker.chunks]) {
    if (!safeRelativePath(asset?.path) || !Number.isSafeInteger(asset?.bytes) || asset.bytes < 1
        || asset.bytes > MAX_ASSET_CHUNK_BYTES || !/^[a-f0-9]{64}$/.test(asset.sha256)) {
      throw new Error(`Invalid or oversized compiler chunk; each chunk must be at most ${MAX_ASSET_CHUNK_BYTES} bytes`);
    }
  }
  return value;
}

async function getManifest(manifestUrl) {
  if (!manifestPromise) {
    manifestPromise = (async () => {
      const base = new URL(manifestUrl);
      if (!['http:', 'https:'].includes(base.protocol) || base.origin !== self.location.origin) {
        throw new Error('Compiler manifest must be served from this page origin');
      }
      const response = await fetch(base);
      if (!response.ok) throw new Error(`Compiler manifest: HTTP ${response.status}`);
      return { value: validateManifest(await response.json(), base), base };
    })();
  }
  return manifestPromise;
}

async function streamChunks(chunks, base, id, label, onChunk) {
  for (let index = 0; index < chunks.length; index++) {
    const bytes = await fetchChecked(chunks[index], base, `${label} part ${index + 1}`);
    await onChunk(bytes, index);
    progress(id, 'downloading', `Downloaded ${label} part ${index + 1} of ${chunks.length}`);
  }
}

async function streamRuntimeTarToGuest(chunks, base, id, {
  maxBytes = MAX_RUNTIME_TAR_BYTES,
  partBytes = MAX_ASSET_CHUNK_BYTES,
} = {}) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 1
      || !Number.isSafeInteger(partBytes) || partBytes < 1 || partBytes > MAX_ASSET_CHUNK_BYTES) {
    throw new Error('Invalid Rust runtime staging limits');
  }
  if (typeof DecompressionStream !== 'function') throw new Error('This browser has no gzip decompression support');
  let next = 0;
  let total = 0;
  let part = 0;
  let used = 0;
  let buffer = null;
  const compressed = new ReadableStream({
    async pull(controller) {
      try {
        if (next >= chunks.length) { controller.close(); return; }
        const index = next++;
        const bytes = await fetchChecked(chunks[index], base, `Rust runtime part ${index + 1}`);
        controller.enqueue(bytes);
        progress(id, 'downloading', `Downloaded Rust runtime part ${index + 1} of ${chunks.length}`);
      } catch (error) { controller.error(error); }
    },
  });
  const reader = compressed.pipeThrough(new DecompressionStream('gzip')).getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (total + value.byteLength > maxBytes) {
        throw new Error(`Rust runtime archive exceeds ${maxBytes} decompressed bytes`);
      }
      total += value.byteLength;
      let offset = 0;
      while (offset < value.byteLength) {
        if (buffer === null) buffer = new Uint8Array(partBytes);
        const copied = Math.min(buffer.byteLength - used, value.byteLength - offset);
        buffer.set(value.subarray(offset, offset + copied), used);
        used += copied;
        offset += copied;
        if (used === buffer.byteLength) {
          await emulator.create_file(`/runtime-tar-${String(part++).padStart(4, '0')}.tar`, buffer);
          buffer = null;
          used = 0;
        }
      }
    }
    if (total === 0) throw new Error('Rust runtime archive is empty');
    if (used > 0) {
      await emulator.create_file(`/runtime-tar-${String(part++).padStart(4, '0')}.tar`, buffer.slice(0, used));
    }
  } catch (error) {
    try { await reader.cancel(error); } catch { /* preserve the original stream or guest-write error */ }
    throw error;
  } finally {
    reader.releaseLock();
  }
  return { bytes: total, parts: part };
}

async function stageRuntimeTar(chunks, base, id, limits) {
  try {
    return await streamRuntimeTarToGuest(chunks, base, id, limits);
  } catch (error) {
    try { await destroyEmulator(); } catch { /* preserve the original runtime staging error */ }
    throw error;
  }
}

function makeHeader(path, size) {
  const encoded = new TextEncoder().encode(path);
  let name = encoded;
  let prefix = new Uint8Array();
  if (encoded.length > 100) {
    let split = -1;
    for (let i = 0; i < encoded.length; i++) {
      if (encoded[i] === 47 && i <= 155 && encoded.length - i - 1 <= 100) split = i;
    }
    if (split < 1) throw new Error(`Source path is too long: ${path}`);
    prefix = encoded.slice(0, split);
    name = encoded.slice(split + 1);
  }
  const header = new Uint8Array(512);
  header.set(name, 0);
  const ascii = (at, length, text) => header.set(new TextEncoder().encode(text), at);
  const octal = (at, length, value) => {
    const text = value.toString(8).padStart(length - 1, '0');
    if (text.length >= length) throw new Error('Source file is too large for tar');
    ascii(at, length, `${text}\0`);
  };
  octal(100, 8, 0o644);
  octal(108, 8, 0);
  octal(116, 8, 0);
  octal(124, 12, size);
  octal(136, 12, Math.floor(Date.now() / 1000));
  header.fill(32, 148, 156);
  header[156] = 48;
  ascii(257, 6, 'ustar\0');
  ascii(263, 2, '00');
  if (prefix.length) header.set(prefix, 345);
  const checksum = header.reduce((total, byte) => total + byte, 0);
  ascii(148, 8, `${checksum.toString(8).padStart(6, '0')}\0 `);
  return header;
}

function countUtf8Bytes(text, limit, label) {
  const encoder = new TextEncoder();
  let bytes = 0;
  for (let offset = 0; offset < text.length;) {
    let end = Math.min(offset + 32_768, text.length);
    if (end < text.length) {
      const last = text.charCodeAt(end - 1);
      if (last >= 0xd800 && last <= 0xdbff) end--;
    }
    bytes += encoder.encode(text.slice(offset, end)).byteLength;
    if (bytes > limit) throw new Error(`${label} exceeds ${limit} UTF-8 bytes`);
    offset = end;
  }
  return bytes;
}

async function sourceArchive(files) {
  const encoder = new TextEncoder();
  const entries = [];
  let total = 0;
  for (const [path, text] of Object.entries(files)) {
    if (!safeRelativePath(path) || typeof text !== 'string') throw new Error(`Invalid crate file ${path}`);
    const size = countUtf8Bytes(text, MAX_SOURCE_BYTES - total, `Crate sources`);
    total += size;
    if (total > MAX_SOURCE_BYTES) throw new Error('Crate sources exceed 16 MiB');
    const bytes = encoder.encode(text);
    if (bytes.length !== size) throw new Error(`Could not encode crate file ${path}`);
    entries.push({ path, bytes });
  }
  if (!entries.some(({ path }) => path === 'src/main.rs')) throw new Error('Crate has no src/main.rs');
  const length = entries.reduce((sum, entry) => sum + 512 + Math.ceil(entry.bytes.length / 512) * 512, 1024);
  const tar = new Uint8Array(length);
  let offset = 0;
  for (const entry of entries) {
    const header = makeHeader(entry.path, entry.bytes.length);
    tar.set(header, offset);
    offset += 512;
    tar.set(entry.bytes, offset);
    offset += Math.ceil(entry.bytes.length / 512) * 512;
  }
  if (typeof CompressionStream !== 'function') throw new Error('This browser has no gzip compression support');
  return new Uint8Array(await new Response(new Blob([tar]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer());
}

function markerMatched(buffer, marker, line = false) {
  return line
    ? buffer.includes(`\n${marker}\n`) || buffer.includes(`\r\n${marker}\r\n`)
    : buffer.includes(marker);
}

function appendSerial(byte) {
  const character = String.fromCharCode(byte);
  serialTail = (serialTail + character).slice(-4096);
  if (serialCapture !== null && serialCaptureLength < MAX_DIAGNOSTICS) {
    serialCapture[serialCaptureLength++] = byte;
  }
  for (const waiter of [...serialWaiters]) {
    if (markerMatched(serialTail, waiter.marker, waiter.line)) {
      serialWaiters = serialWaiters.filter((item) => item !== waiter);
      waiter.resolve();
    }
  }
}

function waitForMarker(marker, timeoutMs, { line = false } = {}) {
  if (markerMatched(serialTail, marker, line)) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const waiter = { marker, line, resolve: () => { clearTimeout(timer); resolve(); } };
    const timer = setTimeout(() => {
      serialWaiters = serialWaiters.filter((item) => item !== waiter);
      reject(new Error(`Linux guest marker ${JSON.stringify(marker)} timed out; serial tail: ${serialTail.slice(-1024)}`));
    }, timeoutMs);
    serialWaiters.push(waiter);
  });
}

async function destroyEmulator() {
  let destroyError = null;
  if (emulator) {
    const current = emulator;
    emulator = null;
    emulatorReady = false;
    try { await current.destroy(); } catch (error) { destroyError = error; }
  }
  if (v86ObjectUrl) {
    URL.revokeObjectURL(v86ObjectUrl);
    v86ObjectUrl = null;
  }
  if (destroyError) throw new Error('Could not release the Linux compiler memory', { cause: destroyError });
}

async function runCommand(script, fileName, timeoutMs, protocolMarkers = []) {
  const nonce = [...crypto.getRandomValues(new Uint8Array(12))]
    .map((byte) => byte.toString(16).padStart(2, '0')).join('');
  const statusMarker = `__BINDGEN_COMMAND_STATUS_${nonce}_`;
  const marker = `__BINDGEN_COMMAND_DONE_${nonce}__`;
  const encoded = new TextEncoder().encode(`${script}\nstatus=$?\nprintf '\\n${statusMarker}%s__\\n' "$status"\nprintf '\\n${marker}\\n'\n`);
  await emulator.create_file(`/${fileName}`, encoded);
  serialCapture = new Uint8Array(MAX_DIAGNOSTICS);
  serialCaptureLength = 0;
  emulator.serial0_send(`sh /mnt/${fileName}\n`);
  await waitForMarker(marker, timeoutMs, { line: true });
  const statusMatch = serialTail.match(new RegExp(`${statusMarker}(\\d+)__`));
  if (!statusMatch) throw new Error(`Linux guest command ${fileName} returned no status marker`);
  const output = new TextDecoder().decode(serialCapture?.subarray(0, serialCaptureLength) ?? new Uint8Array());
  serialCapture = null;
  serialCaptureLength = 0;
  const end = output.lastIndexOf(marker);
  const transcript = (end >= 0 ? output.slice(0, end) : output).replace(/\r?\n$/, '');
  const commandEcho = `sh /mnt/${fileName}`;
  return { output: clamp(transcript.split(/\r?\n/).filter((line) => {
    const trimmed = line.trim();
    return !trimmed.startsWith(commandEcho)
      && !trimmed.endsWith(commandEcho)
      && !['~%', '~% ', '~#', '~ #', '~ $', '#', '$', '%'].includes(trimmed)
      && trimmed !== '__COMPILER_RUNTIME_READY__'
      && trimmed !== '__BINDGEN_SHELL_READY__'
      && !trimmed.startsWith(statusMarker)
      && !trimmed.startsWith('__BINDGEN_COMMAND_STATUS_')
      && !trimmed.startsWith('__BINDGEN_COMMAND_DONE_')
      && !/^\[\s*\d+\.\d+\]\s+rustc \(\d+\) used greatest stack depth: \d+ bytes left$/.test(trimmed)
      || protocolMarkers.includes(trimmed);
  }).join('\n').trim()), exitCode: Number(statusMatch[1]) };
}

async function startEmulator(manifest, base, id) {
  if (emulatorReady) return;
  progress(id, 'downloading', 'Loading Linux compiler assets…');
  const v86ModuleUrl = sameOriginUrl(manifest.v86.module, base, 'v86 module');
  const { V86 } = await import(v86ModuleUrl.href);
  const assetEntries = [
    ['wasm', manifest.v86.wasm], ['kernel', manifest.v86.kernel],
    ['bios', manifest.v86.bios], ['vga BIOS', manifest.v86.vga_bios], ['initrd', manifest.v86.initrd],
  ].filter(([, asset]) => asset);
  const assetBytes = new Map();
  for (const [label, asset] of assetEntries) {
    assetBytes.set(label, await fetchChecked(asset, base, `v86 ${label}`));
    progress(id, 'downloading', `Downloaded v86 ${label}`);
  }
  const wasmBlob = new Blob([assetBytes.get('wasm')], { type: 'application/wasm' });
  v86ObjectUrl = URL.createObjectURL(wasmBlob);
  const options = {
    wasm_path: v86ObjectUrl,
    memory_size: manifest.memory_bytes,
    vga_memory_size: 8 * 1024 * 1024,
    bzimage: { buffer: assetBytes.get('kernel').buffer },
    filesystem: {},
    cmdline: manifest.v86.cmdline || 'tsc=reliable mitigations=off random.trust_cpu=on',
    autostart: true,
    disable_keyboard: true,
    disable_mouse: true,
    disable_speaker: true,
  };
  if (assetBytes.has('bios')) options.bios = { buffer: assetBytes.get('bios').buffer };
  if (assetBytes.has('vga BIOS')) options.vga_bios = { buffer: assetBytes.get('vga BIOS').buffer };
  if (assetBytes.has('initrd')) options.initrd = { buffer: assetBytes.get('initrd').buffer };
  for (const [key, bytes] of assetBytes) assetBytes.set(key, null);
  job.readyMarker = manifest.v86.ready_marker || '~% ';
  if (typeof job.readyMarker !== 'string' || job.readyMarker.length < 1 || job.readyMarker.length > 32) {
    throw new Error('Invalid v86 boot marker');
  }
  if (typeof options.cmdline !== 'string' || options.cmdline.length > 512 || /[\0\r\n]/.test(options.cmdline)) {
    throw new Error('Invalid v86 kernel command line');
  }
  serialTail = '';
  serialWaiters = [];
  emulator = new V86(options);
  emulator.add_listener('serial0-output-byte', appendSerial);
  progress(id, 'booting', 'Starting Linux compiler…');
  await waitForMarker(job.readyMarker, 180_000);
  emulatorReady = true;
  progress(id, 'booting', 'Linux compiler is ready');

  const shellReady = await runCommand('echo __BINDGEN_SHELL_READY__', 'shell-ready.sh', 60_000, ['__BINDGEN_SHELL_READY__']);
  if (shellReady.exitCode !== 0 || !shellReady.output.includes('__BINDGEN_SHELL_READY__')) {
    throw new Error('Linux shell command handshake failed');
  }

  await stageRuntimeTar(manifest.runtime.chunks, base, id);
  await streamChunks(manifest.dependencies.chunks, base, id, 'Rust dependencies', async (bytes, index) => {
    await emulator.create_file(`/dependencies-${String(index).padStart(4, '0')}.gz`, bytes);
  });
  const script = [
    'mkdir -p /tmp/lib /tmp/rust /tmp/deps /tmp/jobs',
    'cat /mnt/runtime-tar-*.tar | tar -xf - -C /tmp',
    'cat /mnt/dependencies-*.gz | gunzip -c | tar -xf - -C /tmp/deps',
    'chmod +x /tmp/lib/ld-linux.so.2',
    '/tmp/lib/ld-linux.so.2 --library-path /tmp/lib:/tmp/rust/lib /tmp/rust/bin/rustc --version',
    'echo __COMPILER_RUNTIME_READY__',
    'rm -f /mnt/runtime-tar-*.tar /mnt/dependencies-*.gz /mnt/compiler-bootstrap.sh',
    '',
  ].join('\n');
  const result = await runCommand(script, 'compiler-bootstrap.sh', 240_000, ['__COMPILER_RUNTIME_READY__']);
  if (result.exitCode !== 0 || !result.output.includes('__COMPILER_RUNTIME_READY__')
      || !result.output.includes('rustc 1.93.0 (254b59607 2026-01-19)')) {
    throw new Error(`Guest compiler setup failed:\n${result.output}`);
  }
}

function shimSource(moduleName) {
  return `\nmod __pyo3_bindgen_entry {\n    use pyo3::ffi;\n    use std::{panic::{self, AssertUnwindSafe}, process::{ExitCode, Termination}, ptr};\n    unsafe extern "C" fn run_main(_self: *mut ffi::PyObject, _args: *mut ffi::PyObject) -> *mut ffi::PyObject {\n        match panic::catch_unwind(AssertUnwindSafe(|| Termination::report(super::main()))) {\n            Ok(ExitCode::SUCCESS) => unsafe { let none = ffi::Py_None(); ffi::Py_IncRef(none); none },\n            Ok(_) => unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"Rust main returned a failure exit code".as_ptr()); ptr::null_mut() },\n            Err(_) => unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, c"Rust main panicked".as_ptr()); ptr::null_mut() },\n        }\n    }\n    static mut METHODS: [ffi::PyMethodDef; 2] = [\n        ffi::PyMethodDef { ml_name: c"run_main".as_ptr(), ml_meth: ffi::PyMethodDefPointer { PyCFunction: run_main }, ml_flags: ffi::METH_NOARGS, ml_doc: c"Run generated Rust main".as_ptr() },\n        ffi::PyMethodDef::zeroed(),\n    ];\n    static mut MODULE_DEF: ffi::PyModuleDef = ffi::PyModuleDef {\n        m_base: ffi::PyModuleDef_HEAD_INIT, m_name: concat!(stringify!(${moduleName}), "\\0").as_ptr().cast(),\n        m_doc: ptr::null(), m_size: 0, m_methods: ptr::addr_of_mut!(METHODS).cast(), m_slots: ptr::null_mut(),\n        m_traverse: None, m_clear: None, m_free: None,\n    };\n    #[unsafe(no_mangle)]\n    pub unsafe extern "C" fn PyInit_${moduleName}() -> *mut ffi::PyObject { unsafe { ffi::PyModuleDef_Init(ptr::addr_of_mut!(MODULE_DEF)) } }\n}\n`;
}

async function guestCompile(files, manifest, base, id, moduleName) {
  const source = await sourceArchive(files);
  const shim = new TextEncoder().encode(shimSource(moduleName));
  const sourceFile = `${moduleName}-source.tar.gz`;
  const shimFile = `${moduleName}-shim.rs`;
  const scriptFile = `${moduleName}-compile.sh`;
  const archiveFile = `${moduleName}.a`;
  await emulator.create_file(`/${sourceFile}`, source);
  await emulator.create_file(`/${shimFile}`, shim);
  const pyo3 = manifest.dependencies.pyo3_rlib;
  const guestScript = [
    `rm -rf /tmp/jobs/${moduleName}`,
    `mkdir -p /tmp/jobs/${moduleName}`,
    `gunzip -c /mnt/${sourceFile} | tar -xf - -C /tmp/jobs/${moduleName}`,
    `cat /tmp/jobs/${moduleName}/src/main.rs /mnt/${shimFile} > /tmp/jobs/${moduleName}/src/main.rs.tmp`,
    `mv /tmp/jobs/${moduleName}/src/main.rs.tmp /tmp/jobs/${moduleName}/src/main.rs`,
    `rm -f /mnt/${archiveFile}`,
    `cd /tmp/jobs/${moduleName}`,
    `if /tmp/lib/ld-linux.so.2 --library-path /tmp/lib:/tmp/rust/lib /tmp/rust/bin/rustc --color never --edition=2024 --crate-name ${moduleName} --crate-type staticlib --emit=link --target wasm32-unknown-emscripten --cfg Py_3_14 -C relocation-model=pic -C panic=unwind -L dependency=/tmp/deps --extern pyo3=/tmp/deps/${pyo3} src/main.rs -o /mnt/${archiveFile}; then compile_status=0; else compile_status=$?; fi`,
    `rm -rf /tmp/jobs/${moduleName}`,
    `rm -f /mnt/${sourceFile} /mnt/${shimFile} /mnt/${scriptFile}`,
    `test "$compile_status" -eq 0`,
    '',
  ].join('\n');
  progress(id, 'compiling', 'Compiling edited Rust in Linux…');
  const result = await runCommand(guestScript, scriptFile, 300_000);
  if (result.exitCode !== 0) {
    return { ok: false, diagnostics: clamp(result.output) };
  }
  const archive = await emulator.read_file(`/${archiveFile}`);
  if (!(archive instanceof Uint8Array) || archive.byteLength < 1 || archive.byteLength > MAX_ARCHIVE_BYTES) {
    throw new Error('Guest Rust compiler returned an invalid static library');
  }
  return { ok: true, archive: archive.slice(), diagnostics: clamp(result.output) };
}

async function getWasiClasses(manifest, base) {
  if (!wasiClassesPromise) {
    wasiClassesPromise = import(sameOriginUrl(manifest.wasi_module, base, 'WASI module').href)
      .then(({ ConsoleStdout, Directory, File, OpenFile, PreopenDirectory, WASI, WASIProcExit }) => {
        if (![ConsoleStdout, Directory, File, OpenFile, PreopenDirectory, WASI, WASIProcExit].every(Boolean)) {
          throw new Error('Browser WASI module is missing required exports');
        }
        return { ConsoleStdout, Directory, File, OpenFile, PreopenDirectory, WASI, WASIProcExit };
      });
  }
  return wasiClassesPromise;
}

async function loadLinker(manifest, base, id) {
  if (!linkerPromise) {
    linkerPromise = (async () => {
      if (typeof DecompressionStream !== 'function') throw new Error('This browser has no gzip decompression support');
      let next = 0;
      const chunks = manifest.linker.chunks;
      const readable = new ReadableStream({
        async pull(controller) {
          try {
            if (next >= chunks.length) { controller.close(); return; }
            const index = next++;
            const bytes = await fetchChecked(chunks[index], base, `browser linker part ${index + 1}`);
            controller.enqueue(bytes);
            progress(id, 'downloading', `Downloaded browser linker part ${index + 1} of ${chunks.length}`);
          } catch (error) { controller.error(error); }
        },
      });
      const raw = new Uint8Array(manifest.linker.bytes);
      const reader = readable.pipeThrough(new DecompressionStream('gzip')).getReader();
      let length = 0;
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          if (length + value.byteLength > raw.byteLength) {
            await reader.cancel();
            throw new Error('Browser linker exceeds its declared size');
          }
          raw.set(value, length);
          length += value.byteLength;
        }
      } finally {
        reader.releaseLock();
      }
      if (length !== raw.byteLength) throw new Error('Browser linker size mismatch');
      const digest = await crypto.subtle.digest('SHA-256', raw);
      const hash = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
      if (hash !== manifest.linker.sha256) throw new Error('Browser linker SHA-256 mismatch');
      progress(id, 'linking', 'Loading browser linker…');
      return WebAssembly.compile(raw);
    })();
  }
  return linkerPromise;
}

async function linkArchive(archive, moduleName, manifest, base, id) {
  progress(id, 'downloading', 'Loading browser linker…');
  const [module, classes] = await Promise.all([loadLinker(manifest, base, id), getWasiClasses(manifest, base)]);
  const { ConsoleStdout, Directory, File, OpenFile, PreopenDirectory, WASI, WASIProcExit } = classes;
  const root = new Directory([
    ['workspace', new Directory([[`${moduleName}.a`, new File(archive)]])],
    ['tmp', new Directory([])],
  ]);
  const stdout = [];
  const stderr = [];
  let outputBytes = 0;
  const captureLinkerOutput = (lines, line) => {
    if (outputBytes >= MAX_DIAGNOSTICS) return;
    const text = String(line);
    const remaining = Math.max(1, MAX_DIAGNOSTICS - outputBytes);
    const kept = text.slice(0, remaining);
    lines.push(kept);
    outputBytes += Math.max(1, kept.length);
  };
  const invoke = async (args) => {
    const wasi = new WASI(args, [], [
      new OpenFile(new File([])),
      ConsoleStdout.lineBuffered((line) => captureLinkerOutput(stdout, line)),
      ConsoleStdout.lineBuffered((line) => captureLinkerOutput(stderr, line)),
      new PreopenDirectory('/', root.contents),
    ]);
    const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
    try { wasi.start(instance); return 0; }
    catch (error) {
      if (error instanceof WASIProcExit) return error.code;
      throw error;
    }
  };
  progress(id, 'linking', 'Linking Python extension…');
  const args = [
    'wasm-ld', '-o', `/workspace/${moduleName}.wasm`, '--strip-debug', '--gc-sections',
    `--export=PyInit_${moduleName}`, '--export=__wasm_call_ctors', '--experimental-pic',
    '--unresolved-symbols=import-dynamic', '-shared', '--no-export-dynamic', '--stack-first',
    `/workspace/${moduleName}.a`,
  ];
  const exitCode = await invoke(args);
  const detail = clamp([...stdout, ...stderr].join('\n'));
  if (exitCode !== 0) throw new Error(`wasm-ld exited with status ${exitCode}${detail ? `:\n${detail}` : ''}`);
  const linked = root.contents.get('workspace').contents.get(`${moduleName}.wasm`);
  if (!(linked instanceof File) || !(linked.data instanceof Uint8Array) || linked.data.byteLength < 8) {
    throw new Error('Browser linker produced no WebAssembly module');
  }
  const result = linked.data.slice();
  const wasmModule = await WebAssembly.compile(result);
  if (!WebAssembly.Module.exports(wasmModule).some(({ name }) => name === `PyInit_${moduleName}`)) {
    throw new Error('Linked side module has no expected Python initializer');
  }
  return result;
}

async function compile(request) {
  const { id, files, manifestUrl } = request;
  const diagnostics = '';
  try {
    if (!Number.isSafeInteger(id) || !manifestUrl) throw new Error('Invalid compiler request');
    progress(id, 'downloading', 'Loading compiler manifest…');
    const { value: manifest, base } = await getManifest(manifestUrl);
    await startEmulator(manifest, base, id);
    const moduleName = `pyo3_bindgen_run_${modulePrefix}_${nextModule++}`;
    const built = await guestCompile(files, manifest, base, id, moduleName);
    if (!built.ok) {
      fail(id, 'Rust compilation failed', built.diagnostics, false);
      return;
    }
    await destroyEmulator();
    const wasm = await linkArchive(built.archive, moduleName, manifest, base, id);
    send(id, {
      type: 'result', moduleName, wasm: wasm.buffer,
      diagnostics: built.diagnostics,
    }, [wasm.buffer]);
  } catch (error) {
    const text = error?.stack || String(error);
    fail(id, error?.message || 'Browser compilation failed', clamp(`${diagnostics}${text}`), true);
  }
}

self.addEventListener('message', ({ data }) => {
  if (data?.type !== 'compile') return;
  if (job) {
    fail(data.id, 'A compile is already running', '', false);
    return;
  }
  job = { id: data.id, readyMarker: '~% ' };
  Promise.resolve(compile(data)).finally(() => { job = null; });
});
