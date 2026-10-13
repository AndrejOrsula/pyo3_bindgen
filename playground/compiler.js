const PHASE_TIMEOUTS = {
  downloading: 180_000,
  booting: 180_000,
  compiling: 300_000,
  linking: 240_000,
};
const MAX_SOURCE_BYTES = 16 * 1024 * 1024;
const IDLE_TIMEOUT = 300_000;

function countUtf8Bytes(text, limit) {
  const encoder = new TextEncoder();
  let bytes = 0;
  for (let offset = 0; offset < text.length;) {
    let end = Math.min(offset + 32_768, text.length);
    if (end < text.length) {
      const last = text.charCodeAt(end - 1);
      if (last >= 0xd800 && last <= 0xdbff) end--;
    }
    bytes += encoder.encode(text.slice(offset, end)).byteLength;
    if (bytes > limit) throw new BrowserCompilerError('Crate sources exceed 16 MiB');
    offset = end;
  }
  return bytes;
}

export class BrowserCompilerError extends Error {
  constructor(message, { diagnostics = '', cancelled = false, cause } = {}) {
    super(message, { cause });
    this.name = 'BrowserCompilerError';
    this.diagnostics = diagnostics;
    this.cancelled = cancelled;
  }
}

export class BrowserCompiler {
  constructor(manifestUrl) {
    this.manifestUrl = new URL(manifestUrl, import.meta.url);
    this.worker = null;
    this.active = null;
    this.idleTimer = null;
    this.nextId = 1;
    this.disposed = false;
  }

  async compile(files, onProgress = () => {}) {
    if (this.disposed) return Promise.reject(new BrowserCompilerError('Compiler was disposed'));
    if (this.active) return Promise.reject(new BrowserCompilerError('A compile is already running'));
    if (!files || typeof files !== 'object' || Array.isArray(files)) {
      return Promise.reject(new BrowserCompilerError('Expected a crate files map'));
    }
    if (typeof files['src/main.rs'] !== 'string') {
      return Promise.reject(new BrowserCompilerError('Crate has no src/main.rs'));
    }
    if (typeof onProgress !== 'function') onProgress = () => {};
    try {
      let total = 0;
      for (const [path, text] of Object.entries(files)) {
        if (path.length < 1 || path.length > 255 || typeof text !== 'string') {
          throw new BrowserCompilerError(`Invalid crate file ${path}`);
        }
        total += countUtf8Bytes(text, MAX_SOURCE_BYTES - total);
        if (total > MAX_SOURCE_BYTES) throw new BrowserCompilerError('Crate sources exceed 16 MiB');
      }
    } catch (error) {
      return Promise.reject(error instanceof BrowserCompilerError
        ? error
        : new BrowserCompilerError('Could not inspect crate sources', { cause: error }));
    }

    this.clearIdleTimer();
    const id = this.nextId++;
    const worker = this.worker ?? new Worker(new URL('./compiler-worker.js', import.meta.url), { type: 'module' });
    this.worker = worker;

    return new Promise((resolve, reject) => {
      const active = { id, worker, resolve, reject, onProgress, timer: null, settled: false };
      this.active = active;
      const armTimeout = (phase) => {
        clearTimeout(active.timer);
        active.timer = setTimeout(() => {
          this.fail(active, new BrowserCompilerError(`Compiler ${phase} timed out`, {
            diagnostics: active.diagnostics ?? '',
          }), true);
        }, PHASE_TIMEOUTS[phase] ?? 300_000);
      };
      active.armTimeout = armTimeout;
      armTimeout('downloading');

      const onMessage = ({ data }) => {
        if (this.active !== active || data?.id !== id) return;
        if (data.type === 'progress') {
          if (PHASE_TIMEOUTS[data.phase]) armTimeout(data.phase);
          try { onProgress({ phase: data.phase, text: String(data.text ?? '') }); } catch { /* UI progress cannot fail compilation */ }
          return;
        }
        if (data.type === 'result') {
          if (!(data.wasm instanceof ArrayBuffer)) {
            this.fail(active, new BrowserCompilerError('Compiler returned no module bytes'), true);
            return;
          }
          this.finish(active);
          this.armIdleTimer(worker);
          resolve({
            wasm: new Uint8Array(data.wasm),
            moduleName: String(data.moduleName),
            diagnostics: String(data.diagnostics ?? '').slice(-65_536),
          });
          return;
        }
        if (data.type === 'error') {
          const error = new BrowserCompilerError(String(data.message ?? 'Compilation failed'), {
            diagnostics: String(data.diagnostics ?? '').slice(-65_536),
            cancelled: Boolean(data.cancelled),
          });
          this.fail(active, error, Boolean(data.fatal));
        }
      };
      const onError = (event) => {
        this.fail(active, new BrowserCompilerError(event.message || 'Compiler worker failed', {
          diagnostics: active.diagnostics ?? '',
        }), true);
      };
      active.onMessage = onMessage;
      active.onError = onError;
      worker.addEventListener('message', onMessage);
      worker.addEventListener('error', onError);
      try {
        worker.postMessage({ type: 'compile', id, manifestUrl: this.manifestUrl.href, files });
      } catch (error) {
        this.fail(active, new BrowserCompilerError('Could not send crate sources to the compiler worker', {
          cause: error,
        }), true);
      }
    });
  }

  cancel() {
    if (this.active) {
      this.fail(this.active, new BrowserCompilerError('Compilation cancelled', {
        diagnostics: this.active.diagnostics ?? '', cancelled: true,
      }), true);
    } else {
      this.stopWorker();
    }
  }

  dispose() {
    this.disposed = true;
    this.cancel();
    this.stopWorker();
  }

  finish(active) {
    if (active.settled) return;
    active.settled = true;
    clearTimeout(active.timer);
    active.worker.removeEventListener('message', active.onMessage);
    active.worker.removeEventListener('error', active.onError);
    if (this.active === active) this.active = null;
  }

  fail(active, error, terminate) {
    if (active.settled) return;
    this.finish(active);
    if (terminate) this.stopWorker();
    else this.armIdleTimer(active.worker);
    active.reject(error);
  }

  armIdleTimer(worker) {
    this.clearIdleTimer();
    const idle = { worker, timer: null };
    idle.timer = setTimeout(() => {
      if (this.idleTimer !== idle || this.worker !== worker || this.active !== null) return;
      this.idleTimer = null;
      worker.terminate();
      this.worker = null;
    }, IDLE_TIMEOUT);
    this.idleTimer = idle;
  }

  clearIdleTimer() {
    const idle = this.idleTimer;
    this.idleTimer = null;
    if (idle) clearTimeout(idle.timer);
  }

  stopWorker() {
    this.clearIdleTimer();
    this.worker?.terminate();
    this.worker = null;
  }
}
