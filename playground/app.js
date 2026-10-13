import { highlightMarkdown, highlightPython, highlightRust, highlightToml } from './highlight.js';
import { overviewText, parseBindings, pythonLines, renderOverview } from './overview.js';
import { DEFAULT_EXAMPLE, EXAMPLES, selectStarterBody } from './examples.js';
import { BrowserCompiler } from './compiler.js';

// The id under which the author's portfolio embeds this page; see README.md.
const DEMO_ID = 'pyo3_bindgen';
// Highlighting larger outputs would make the page sluggish; they stay plain text.
const HIGHLIGHT_LIMIT = 400_000;
// Links longer than this leave the pasted code out; most chat and mail clients
// keep links of this length intact.
const LINK_CODE_LIMIT = 16_000;
const LINK_SOURCE_BYTE_LIMIT = 1_048_576;
const LINK_SOURCE_LINE_LIMIT = 10_000;
// The editor colours pasted code up to this length.
const EDITOR_HIGHLIGHT_LIMIT = 200_000;
// Live generation waits for a pause in typing.
const LIVE_DELAY = 700;
const DEFAULT_FILE = 'src/bindings.rs';
const $ = (id) => document.getElementById(id);
const ui = {
  status: $('status'),
  statusText: $('status-text'),
  example: $('example'),
  exampleGuide: $('example-guide'),
  exampleDescription: $('example-description'),
  exampleConcepts: $('example-concepts'),
  exampleExperiment: $('example-experiment'),
  exampleExperimentDescription: $('example-experiment-description'),
  exampleResult: $('example-result'),
  exampleReset: $('example-reset'),
  exampleState: $('example-state'),
  source: $('source'),
  editor: $('editor'),
  sourceColors: $('source-colors'),
  sourceGutter: $('source-gutter'),
  sourceError: $('source-error'),
  sourceFocus: $('source-focus'),
  rustEditorPanel: $('rust-editor-panel'),
  rustSource: $('rust-source'),
  rustColors: $('rust-colors'),
  rustGutter: $('rust-gutter'),
  rustReset: $('rust-reset'),
  rustEditorHelp: $('rust-editor-help'),
  sourceModule: $('source-module'),
  requirement: $('requirement'),
  installForm: $('install-form'),
  install: $('install'),
  installed: $('installed'),
  installedTitle: $('installed-title'),
  installedModules: $('installed-modules'),
  modules: $('modules'),
  options: $('options'),
  optionsChanged: $('options-changed'),
  optionsReset: $('options-reset'),
  blocklist: $('blocklist'),
  generate: $('generate'),
  cancel: $('cancel'),
  retry: $('retry'),
  live: $('live'),
  views: $('views'),
  files: $('files'),
  copy: $('copy'),
  showExampleCall: $('show-example-call'),
  compileRun: $('compile-run'),
  compilerCost: $('compiler-cost'),
  compilerNotices: $('compiler-notices'),
  runConsole: $('run-console'),
  runProgress: $('run-progress'),
  compileOutputSection: $('compile-output-section'),
  compileOutput: $('compile-output'),
  stdoutSection: $('stdout-section'),
  runStdout: $('run-stdout'),
  stderrSection: $('stderr-section'),
  runStderr: $('run-stderr'),
  runError: $('run-error'),
  cancelRun: $('cancel-run'),
  restartPython: $('restart-python'),
  download: $('download'),
  toolbar: $('toolbar'),
  jump: $('jump'),
  outline: $('outline'),
  toolbarNote: $('toolbar-note'),
  codeWrap: $('code-wrap'),
  codeGrid: $('code-grid'),
  gutter: $('gutter'),
  code: $('code-text'),
  lineMark: $('line-mark'),
  view: $('view'),
  empty: $('empty'),
  steps: $('steps'),
  emptyText: $('empty-text'),
  stale: $('stale'),
  message: $('message'),
  note: $('note'),
  stats: $('stats'),
  warnings: $('warnings'),
  buildInfo: $('build-info'),
};

const state = {
  config: null,
  defaults: null,
  mode: 'source',
  result: null,
  resultOptions: null,
  view: 'overview',
  file: DEFAULT_FILE,
  busy: false,
  ready: false,
  installed: null,
  stale: false,
  generated: false,
  editedBeforeReady: false,
  preReadySnapshot: null,
  examples: new Map(),
  guideExample: null,
  exampleLoading: false,
  rustDraft: '',
  rustRevision: 0,
  rustEdited: false,
  generationId: 0,
  runBusy: false,
  runPhase: null,
  resetRequired: false,
  rustLoadAttempts: 0,
  compilerStarted: false,
};

/* ---------- Embedding protocol ---------- */

function notifyHost(type, extra = {}) {
  if (window.parent === window) return;
  const message = { type, protocol: 1, id: DEMO_ID, ...extra };
  window.parent.postMessage(message, location.origin);
}

/* ---------- Worker ---------- */

let worker = null;
let nextId = 1;
const pending = new Map();
let actionEpoch = 0;
let workerGeneration = 0;
let bootGeneration = 0;
let startupBusy = false;
let initial = null;
let inputRestored = false;
let initialOptionsApplied = false;
let hostReadySent = false;
let startupRetries = 0;
let packagesLost = false;
let exampleSelectionGeneration = 0;
let userEditRevision = 0;
let pendingExampleGeneration = null;
let browserCompiler = null;
let runEpoch = 0;
let runTimeout = null;

function startWorker() {
  const instance = new Worker('worker.js', { type: 'module' });
  const generation = ++workerGeneration;
  worker = instance;
  instance.onmessage = ({ data }) => {
    if (worker !== instance || generation !== workerGeneration) return;
    if (data.type === 'progress') {
      setStatus('loading', data.text);
      setStep(data.step);
      return;
    }
    const request = pending.get(data.id);
    if (data.type === 'run-output') {
      request?.onStream?.(data);
      return;
    }
    if (!request) return;
    pending.delete(data.id);
    if (data.ok) request.resolve({ result: data.result, ms: data.ms });
    else request.reject(Object.assign(new Error(data.error), { ms: data.ms }));
  };
  instance.onerror = (event) => {
    if (worker !== instance || generation !== workerGeneration) return;
    event.preventDefault();
    crashed(instance, event.message || 'The Python worker stopped unexpectedly.');
  };
}

function stopWorker(error = new Error('Python worker replaced.')) {
  workerGeneration += 1;
  const previous = worker;
  worker = null;
  previous?.terminate();
  failAll(error);
}

function crashed(instance, text) {
  if (worker !== instance) return;
  stopWorker(new Error(text));
  state.ready = false;
  state.rustLoadAttempts = 0;
  state.resetRequired = false;
  packagesLost ||= Boolean(state.installed);
  state.installed = null;
  ui.installed.hidden = true;
  markStale(Boolean(state.result));
  setStatus('error', 'Python stopped');
  showRetry(true);
  showMessage(`Python stopped unexpectedly: ${text} Retry to start Python again. Packages installed earlier need installing again.`);
  updateButtons();
  notifyHost('wasm-demo-error', { ...(state.config?.build ? { build: state.config.build } : {}), message: text });
}

function failAll(error) {
  for (const request of pending.values()) request.reject(error);
  pending.clear();
}

function call(command, args, { onStream } = {}) {
  return new Promise((resolve, reject) => {
    if (!worker) {
      reject(new Error('Python is not running. Retry startup before generating.'));
      return;
    }
    const id = nextId++;
    pending.set(id, { resolve, reject, onStream });
    try {
      worker.postMessage({ id, command, args });
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

async function boot() {
  const generation = ++bootGeneration;
  stopWorker();
  state.ready = false;
  state.rustLoadAttempts = 0;
  state.resetRequired = false;
  updateButtons();
  setStatus('loading', 'Starting Python…');
  setStep('python');
  startWorker();
  let result;
  try {
    ({ result } = await call('init', {
      pyodideUrl: state.config.pyodide_url,
      wheelUrl: state.config.wheel,
      glueUrl: 'playground.py',
      generatorRevision: state.config.generator_revision ?? '',
    }));
  } catch (error) {
    if (generation !== bootGeneration) throw Object.assign(error, { cancelled: true });
    stopWorker(error);
    throw error;
  }
  if (generation !== bootGeneration) throw Object.assign(new Error('Startup was superseded.'), { cancelled: true });
  state.defaults ??= result.defaults;
  state.ready = true;
  setStep('generate');
  ui.status.dataset.readyText = `Ready · Python ${result.python}`;
  setStatus('ready', ui.status.dataset.readyText);
  showRetry(false);
  updateButtons();
  return result;
}

/* ---------- Status and messages ---------- */

function setStatus(kind, text) {
  ui.status.dataset.state = kind;
  ui.statusText.textContent = text;
}

// The steps that the empty output lists while the page gets ready.
const STEPS = ['python', 'generator', 'generate'];

function setStep(step, failed = false) {
  const current = STEPS.indexOf(step);
  for (const item of ui.steps.children) {
    const index = STEPS.indexOf(item.dataset.step);
    item.dataset.state = index < current ? 'done' : index > current ? 'pending' : failed ? 'failed' : 'active';
  }
}

function failStep() {
  const active = ui.steps.querySelector('[data-state="active"]');
  if (active) setStep(active.dataset.step, true);
}

function showMessage(text) {
  ui.message.hidden = !text;
  ui.message.textContent = text ?? '';
}

function showNote(text) {
  ui.note.hidden = !text;
  ui.note.textContent = text ?? '';
}

function updateButtons() {
  const idle = state.ready && !state.busy && !state.runBusy;
  ui.generate.disabled = !idle;
  ui.install.disabled = !idle;
  ui.cancel.hidden = !state.busy;
  ui.options.querySelector('.option-groups').inert = !state.defaults;
  const usable = Boolean(state.result) && !state.stale;
  ui.copy.disabled = !usable;
  ui.download.disabled = !usable || !idle;
  ui.showExampleCall.disabled = !usable || !state.result.files['src/main.rs'];
  ui.compileRun.disabled = !usable || !idle || state.runBusy || state.resetRequired || !state.config?.compiler || !state.result.files['src/main.rs'];
  ui.compileRun.textContent = state.runBusy ? 'Compiling and running…' : 'Compile and run';
  ui.cancelRun.hidden = !state.runBusy;
  ui.restartPython.hidden = !state.resetRequired;
  const target = state.view === 'crate' ? state.file : state.view === 'types' ? 'Type map' : state.view === 'report' ? 'Report' : 'Overview';
  ui.copy.textContent = `Copy ${target}`;
  ui.copy.title = usable ? `Copy the current ${target} view.` : 'Generate current bindings before copying.';
}

function showRetry(visible) {
  ui.retry.hidden = !visible;
  ui.retry.disabled = false;
}

function markUserEdit(semantic = true) {
  if (semantic) {
    userEditRevision += 1;
    pendingExampleGeneration = null;
    invalidateActiveRun('Python inputs or generation options changed.');
  }
  if (state.ready) {
    if (semantic && state.result) markStale(true);
    return;
  }
  state.editedBeforeReady = true;
  state.preReadySnapshot = {
    mode: state.mode,
    source: ui.source.value,
    sourceModule: ui.sourceModule.value,
    example: ui.example.value,
    requirement: ui.requirement.value,
    modules: ui.modules.value,
    live: ui.live.checked,
  };
}

function restorePreReadySnapshot() {
  const snapshot = state.preReadySnapshot;
  if (!snapshot) return;
  selectMode(snapshot.mode);
  setSource(snapshot.source);
  ui.sourceModule.value = snapshot.sourceModule;
  ui.example.value = snapshot.example;
  ui.requirement.value = snapshot.requirement;
  ui.modules.value = snapshot.modules;
  ui.live.checked = snapshot.live;
  state.guideExample = snapshot.mode === 'source' && snapshot.example && exampleBase(snapshot.example) === snapshot.source ? snapshot.example : null;
  updateExampleGuide();
}

/* ---------- Tabs ---------- */

function setupTabs(list, onSelect) {
  list.addEventListener('click', (event) => {
    const tab = event.target.closest('[role="tab"]');
    if (tab) onSelect(tab);
  });
  list.addEventListener('keydown', (event) => {
    const tabs = [...list.querySelectorAll('[role="tab"]:not([hidden])')];
    const index = tabs.indexOf(document.activeElement);
    if (index < 0) return;
    const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
    let target = null;
    if (step) target = tabs[(index + step + tabs.length) % tabs.length];
    if (event.key === 'Home') target = tabs[0];
    if (event.key === 'End') target = tabs[tabs.length - 1];
    if (!target) return;
    event.preventDefault();
    target.focus();
    onSelect(target);
  });
}

function selectTab(list, tab) {
  for (const other of list.querySelectorAll('[role="tab"]')) {
    const selected = other === tab;
    other.setAttribute('aria-selected', String(selected));
    other.tabIndex = selected ? 0 : -1;
  }
}

function selectMode(mode) {
  state.mode = mode;
  const tab = $(`tab-${mode}`);
  selectTab(tab.parentElement, tab);
  for (const name of ['source', 'package', 'module']) $(`panel-${name}`).hidden = name !== mode;
  if (ui.exampleGuide) updateExampleGuide();
}

/* ---------- Editor ---------- */

// The source shows in colour through a transparent textarea over a highlighted
// copy. The copy sizes both, and the editor scrolls them together, so nothing
// follows the scroll position in script.
function renderEditor() {
  const text = ui.source.value;
  if (text.length <= EDITOR_HIGHLIGHT_LIMIT) ui.sourceColors.replaceChildren(highlightPython(text));
  else ui.sourceColors.textContent = text;
  // A final newline starts a line that the textarea shows and a `pre` drops.
  if (text.endsWith('\n')) ui.sourceColors.append(' ');
  const lines = text.split('\n').length;
  if (Number(ui.sourceGutter.dataset.lines) !== lines) {
    ui.sourceGutter.dataset.lines = String(lines);
    ui.sourceGutter.textContent = Array.from({ length: lines }, (_, index) => index + 1).join('\n');
  }
  ui.sourceError.hidden = true;
  ui.sourceFocus.hidden = true;
}

// The textarea is as large as its copy plus a spare line and columns, so it
// never scrolls itself; the editor scrolls instead, which this keeps on the
// caret (or the moving end of a selection) as the visitor types or selects.
let characterWidth = 0;
let rustCharacterWidth = 0;
function revealCaret() {
  const { source, editor } = ui;
  if (document.activeElement !== source) return;
  source.scrollTo(0, 0);
  const style = getComputedStyle(source);
  if (!characterWidth) {
    const context = document.createElement('canvas').getContext('2d');
    context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    characterWidth = context.measureText('0'.repeat(32)).width / 32;
  }
  const end = source.selectionDirection === 'backward' ? source.selectionStart : source.selectionEnd;
  const before = source.value.slice(0, end);
  const lineStart = before.lastIndexOf('\n') + 1;
  let line = 0;
  for (let index = before.indexOf('\n'); index !== -1; index = before.indexOf('\n', index + 1)) line += 1;
  let column = 0;
  for (const character of before.slice(lineStart)) column = character === '\t' ? column - (column % 4) + 4 : column + 1;
  const height = parseFloat(style.lineHeight);
  const top = parseFloat(style.paddingTop) + line * height;
  if (top < editor.scrollTop) editor.scrollTop = top - parseFloat(style.paddingTop);
  else if (top + height > editor.scrollTop + editor.clientHeight) editor.scrollTop = top + height - editor.clientHeight + parseFloat(style.paddingTop);
  // The line numbers stay in place over the code's left edge.
  const gutter = ui.sourceGutter.offsetWidth;
  const left = gutter + parseFloat(style.paddingLeft) + column * characterWidth;
  if (left < editor.scrollLeft + gutter + parseFloat(style.paddingLeft)) editor.scrollLeft = left - gutter - parseFloat(style.paddingLeft);
  else if (left + characterWidth > editor.scrollLeft + editor.clientWidth) editor.scrollLeft = left + 2 * characterWidth - editor.clientWidth;
}

function setSource(text) {
  ui.source.value = text;
  renderEditor();
  ui.editor.scrollTo(0, 0);
}

// Marks a line of the pasted code: the line that Python reported an error on,
// or the definition of the item that the overview points at, which scrolls
// into view if it is outside.
function markSourceLine(line, marker = ui.sourceError) {
  if (!line) {
    marker.hidden = true;
    return;
  }
  const style = getComputedStyle(ui.source);
  const height = parseFloat(style.lineHeight);
  const top = parseFloat(style.paddingTop) + (line - 1) * height;
  marker.style.top = `${top}px`;
  marker.style.height = `${height}px`;
  marker.hidden = false;
  const { editor } = ui;
  if (marker === ui.sourceFocus && (top < editor.scrollTop || top + height > editor.scrollTop + editor.clientHeight)) {
    editor.scrollTop = Math.max(0, top - editor.clientHeight / 3);
  }
}

/* ---------- Options ---------- */

const optionInputs = () => [...document.querySelectorAll('[data-option]')];

function setOptions(values) {
  for (const input of optionInputs()) {
    const value = values[input.dataset.option];
    if (value === undefined) continue;
    if (input.type === 'checkbox') input.checked = value;
    else input.value = value.join(' ');
  }
  updateOptionsSummary();
}

function readOptions() {
  const values = {};
  for (const input of optionInputs()) {
    values[input.dataset.option] =
      input.type === 'checkbox' ? input.checked : input.value.split(/[\s,]+/).filter(Boolean);
  }
  return values;
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

function changedOptions(values = readOptions()) {
  if (!state.defaults) return {};
  return Object.fromEntries(Object.entries(values).filter(([key, value]) => !same(value, state.defaults[key])));
}

function updateOptionsSummary() {
  const count = Object.keys(changedOptions()).length;
  ui.optionsChanged.textContent = count ? `${count} changed` : '';
  ui.optionsReset.hidden = count === 0;
}

// The link records changed options: `opt=call_builders,no-generate_docs` and,
// for the skipped names, `skip=a b`.
function optionsToParams(params, values) {
  const flags = [];
  for (const [key, value] of Object.entries(changedOptions(values))) {
    if (key === 'blocklist_names') params.set('skip', value.join(' '));
    else flags.push(value ? key : `no-${key}`);
  }
  if (flags.length) params.set('opt', flags.join(','));
}

function optionsFromParams(params) {
  const values = { ...state.defaults };
  for (const flag of (params.get('opt') ?? '').split(',').filter(Boolean)) {
    const key = flag.replace(/^no-/, '');
    if (typeof values[key] === 'boolean') values[key] = !flag.startsWith('no-');
  }
  if (params.has('skip')) values.blocklist_names = params.get('skip').split(/\s+/).filter(Boolean);
  setOptions(values);
}

/* ---------- Examples ---------- */

async function exampleSource(name) {
  if (!state.examples.has(name)) {
    const response = await fetch(`examples/${name}.py`);
    if (!response.ok) throw new Error(`examples/${name}.py: HTTP ${response.status}`);
    state.examples.set(name, await response.text());
  }
  return state.examples.get(name);
}

async function matchingExperiment(source, module) {
  if (!Object.hasOwn(EXAMPLES, module) || !EXAMPLES[module].experiment) return '';
  try {
    const base = await exampleSource(module);
    return source === `${base.trimEnd()}\n\n${EXAMPLES[module].experiment.append}` ? module : '';
  } catch {
    return '';
  }
}

function matchingExample() {
  for (const [name, text] of state.examples) {
    if (text === ui.source.value && ui.sourceModule.value === name) return name;
  }
  return '';
}

async function loadExample(name, { isCurrent = () => true, applyOptions = () => true } = {}) {
  const source = await exampleSource(name);
  if (!isCurrent()) return false;
  setSource(source);
  ui.sourceModule.value = name;
  ui.example.value = name;
  state.guideExample = name;
  const options = EXAMPLES[name].options;
  if (options && state.defaults && applyOptions()) setOptions({ ...readOptions(), ...options });
  updateExampleGuide();
  return true;
}

function exampleBase(name = state.guideExample) {
  return name ? state.examples.get(name) : undefined;
}

function updateExampleGuide() {
  const name = state.guideExample;
  const example = name && Object.hasOwn(EXAMPLES, name) ? EXAMPLES[name] : null;
  const base = exampleBase(name);
  const experimentSource = example?.experiment && base
    ? `${base.trimEnd()}\n\n${example.experiment.append}`
    : '';
  const pristine = Boolean(base && ui.sourceModule.value === name && ui.source.value === base);
  const applied = Boolean(experimentSource && ui.sourceModule.value === name && ui.source.value === experimentSource);
  const guarded = startupBusy || state.busy || state.exampleLoading;
  ui.exampleGuide.hidden = !example || state.mode !== 'source';
  if (!example) {
    ui.exampleGuide.removeAttribute('data-example');
    return;
  }
  ui.exampleGuide.dataset.example = name;
  ui.exampleDescription.textContent = example.description;
  ui.exampleConcepts.replaceChildren(...example.concepts.map((concept) => {
    const item = document.createElement('li');
    item.textContent = concept;
    return item;
  }));
  ui.exampleExperiment.textContent = example.experiment.label;
  ui.exampleExperimentDescription.textContent = `${example.experiment.description} Expected Rust change: ${example.experiment.item}.`;
  ui.exampleExperiment.disabled = guarded || !pristine;
  ui.exampleReset.disabled = guarded || pristine;
  const resultIsFresh = applied && state.ready && !guarded && !state.stale && state.mode === 'source' &&
    state.result?.source === ui.source.value && state.result?.module === ui.sourceModule.value.trim() &&
    same(state.resultOptions, readOptions()) &&
    outline(state.result?.files[DEFAULT_FILE] ?? '').some((item) => item.name === example.experiment.item);
  ui.exampleResult.textContent = `View ${example.experiment.item} in Rust`;
  ui.exampleResult.hidden = !resultIsFresh;
  ui.exampleResult.disabled = !resultIsFresh;
  if (pristine) {
    ui.exampleState.textContent = 'Original example';
  } else if (!applied) {
    ui.exampleState.textContent = 'Source modified · reset restores the original example';
  } else {
    const currentResult = state.mode === 'source' && state.result && !state.stale &&
      state.result.source === ui.source.value && state.result.module === ui.sourceModule.value.trim() &&
      same(state.resultOptions, readOptions());
    const itemInCurrentResult = currentResult && outline(state.result.files[DEFAULT_FILE] ?? '')
      .some((item) => item.name === example.experiment.item);
    if (itemInCurrentResult) {
      ui.exampleState.textContent = `Experiment applied · generated Rust includes ${example.experiment.item}`;
    } else if (currentResult) {
      const options = readOptions();
      const requiredOption = example.experiment.item === 'Rectangle' ? 'generate_classes' : 'generate_functions';
      const optionLabel = requiredOption === 'generate_classes' ? 'Classes' : 'Functions';
      const omitted = [];
      if (!options[requiredOption]) omitted.push(`${optionLabel} is off`);
      if (options.blocklist_names.includes(example.experiment.item)) omitted.push(`Skip names includes ${example.experiment.item}`);
      ui.exampleState.textContent = omitted.length
        ? `Experiment applied · current options omit ${example.experiment.item}: ${omitted.join('; ')}`
        : `Experiment applied · current Rust output does not include ${example.experiment.item}; review generation options`;
    } else if (!ui.message.hidden) {
      ui.exampleState.textContent = 'Generation failed · fix the source, then generate again';
    } else if (state.busy) {
      ui.exampleState.textContent = `Generating Rust with ${example.experiment.item}…`;
    } else if (ui.live.checked && (livePending || liveTimer !== null)) {
      ui.exampleState.textContent = 'Updated Rust generation queued';
    } else if (!state.ready) {
      ui.exampleState.textContent = 'Experiment applied · Python must be ready to generate Rust';
    } else if (ui.live.checked) {
      ui.exampleState.textContent = 'Experiment applied · output is stale; waiting for generation';
    } else {
      ui.exampleState.textContent = `Experiment applied · choose Generate Rust to add ${example.experiment.item}`;
    }
  }
}

async function resetExample() {
  const name = state.guideExample;
  if (!name || startupBusy || state.busy || state.exampleLoading || !exampleBase(name)) return;
  const pristine = ui.sourceModule.value === name && ui.source.value === exampleBase(name);
  if (pristine) return;
  if (!confirm(`Replace your modified code with the original ${EXAMPLES[name].title} example?`)) return;
  const selection = ++exampleSelectionGeneration;
  const startingMode = state.mode;
  const startingSource = ui.source.value;
  const startingModule = ui.sourceModule.value;
  const startingOptions = readOptions();
  state.exampleLoading = true;
  updateExampleGuide();
  clearTimeout(liveTimer);
  liveTimer = null;
  livePending = false;
  pendingExampleGeneration = null;
  try {
    const loaded = await loadExample(name, {
      isCurrent: () => selection === exampleSelectionGeneration && state.mode === startingMode && ui.source.value === startingSource && ui.sourceModule.value === startingModule,
      applyOptions: () => same(readOptions(), startingOptions),
    });
    if (loaded && selection === exampleSelectionGeneration && ui.source.value === exampleBase(name)) {
      markUserEdit();
      if (ui.live.checked) scheduleLive(0);
      else markStale(Boolean(state.result));
    }
  } catch (error) {
    showMessage(`The example could not load (${error.message}).`);
  } finally {
    if (selection === exampleSelectionGeneration) {
      state.exampleLoading = false;
      updateExampleGuide();
    }
  }
}

function tryExampleExperiment() {
  const name = state.guideExample;
  const example = EXAMPLES[name];
  const base = exampleBase(name);
  if (!example?.experiment || startupBusy || state.busy || state.exampleLoading || !base) return;
  if (ui.sourceModule.value !== name || ui.source.value !== base) return;
  clearTimeout(liveTimer);
  liveTimer = null;
  livePending = false;
  markUserEdit();
  setSource(`${base.trimEnd()}\n\n${example.experiment.append}`);
  ui.example.value = '';
  updateExampleGuide();
  if (ui.live.checked) scheduleLive(0);
  else markStale(Boolean(state.result));
}

function viewExampleResult() {
  const name = state.guideExample;
  const example = EXAMPLES[name];
  const base = exampleBase(name);
  const source = state.result?.files[DEFAULT_FILE];
  const experimentSource = base && example?.experiment ? `${base.trimEnd()}\n\n${example.experiment.append}` : '';
  if (!example?.experiment || ui.exampleResult.hidden || ui.exampleResult.disabled || !source || !state.ready || state.mode !== 'source' || state.busy || startupBusy || state.stale) return;
  if (ui.source.value !== experimentSource || ui.sourceModule.value !== name) return;
  if (state.result.source !== ui.source.value || state.result.module !== ui.sourceModule.value.trim() || !same(state.resultOptions, readOptions())) return;
  const item = outline(source).find((candidate) => candidate.name === example.experiment.item);
  if (!item) return;
  const previousView = state.view;
  showBindingsLine(item.line, item.name, previousView);
  ui.codeWrap.focus({ preventScroll: true });
}

async function chooseExample() {
  const name = ui.example.value;
  const current = ui.source.value.trim();
  if (current && !matchingExample() && !confirm(`Replace your code with the ${EXAMPLES[name].title} example?`)) {
    ui.example.value = '';
    return;
  }
  markUserEdit();
  const selection = ++exampleSelectionGeneration;
  state.exampleLoading = true;
  updateExampleGuide();
  const startingMode = state.mode;
  const startingSource = ui.source.value;
  const startingModule = ui.sourceModule.value;
  const startingOptions = readOptions();
  try {
    const loaded = await loadExample(name, {
      isCurrent: () => selection === exampleSelectionGeneration && state.mode === startingMode && ui.source.value === startingSource && ui.sourceModule.value === startingModule,
      applyOptions: () => same(readOptions(), startingOptions),
    });
    if (!loaded) {
      if (selection === exampleSelectionGeneration) ui.example.value = matchingExample();
      return;
    }
    if (state.busy) {
      pendingExampleGeneration = { selection, editRevision: userEditRevision };
      return;
    }
  } catch (error) {
    showMessage(`The example could not load (${error.message}).`);
    return;
  } finally {
    if (selection === exampleSelectionGeneration) {
      state.exampleLoading = false;
      updateExampleGuide();
    }
  }
  await generate();
  if (EXAMPLES[name].note) showNote(EXAMPLES[name].note);
}

/* ---------- Output ---------- */

function renderTabs() {
  ui.files.replaceChildren();
  if (!state.result) return;
  for (const [index, path] of Object.keys(state.result.files).entries()) {
    const tab = document.createElement('button');
    tab.type = 'button';
    tab.id = `file-tab-${index}`;
    tab.setAttribute('role', 'tab');
    tab.setAttribute('aria-controls', ui.codeWrap.id);
    tab.dataset.path = path;
    tab.textContent = path.split('/').pop();
    tab.title = path;
    ui.files.append(tab);
  }
  ui.views.hidden = false;
  $('view-types').hidden = !state.result.analysis;
  if (!(state.file in state.result.files)) state.file = DEFAULT_FILE;
  showView(state.view === 'types' && !state.result.analysis ? 'overview' : state.view);
}

function clearOutput() {
  state.result = null;
  state.resultOptions = null;
  state.stale = false;
  ui.views.hidden = true;
  ui.files.replaceChildren();
  ui.codeWrap.setAttribute('aria-labelledby', 'output-title');
  ui.code.textContent = '';
  ui.gutter.textContent = '';
  ui.codeGrid.hidden = true;
  ui.rustEditorPanel.hidden = true;
  ui.rustEditorHelp.hidden = true;
  ui.rustReset.hidden = true;
  ui.view.hidden = true;
  ui.toolbar.hidden = true;
  ui.empty.hidden = false;
  ui.steps.hidden = state.generated;
  ui.emptyText.hidden = !state.generated;
  ui.stale.hidden = true;
  ui.codeWrap.classList.remove('is-stale');
  ui.stats.textContent = '';
  ui.warnings.hidden = true;
  updateExampleGuide();
  updateButtons();
}

// The crate view shows one file of the crate; the other views summarize the run
// and lead into it.
function showView(view) {
  if (view === 'crate') {
    showFile(state.file);
    return;
  }
  state.view = view;
  const tab = $(`view-${view}`);
  ui.sourceFocus.hidden = true;
  selectTab(ui.views, tab);
  ui.codeWrap.setAttribute('aria-labelledby', tab.id);
  ui.empty.hidden = true;
  ui.lineMark.hidden = true;
  ui.codeGrid.hidden = true;
  ui.rustEditorPanel.hidden = true;
  ui.rustEditorHelp.hidden = true;
  ui.rustReset.hidden = true;
  ui.toolbar.hidden = true;
  ui.view.hidden = false;
  ui.view.replaceChildren({ overview: overviewView, types: typesView, report: reportView }[view]());
  ui.codeWrap.scrollTo(0, 0);
  updateButtons();
}

function showFile(path) {
  state.view = 'crate';
  ui.sourceFocus.hidden = true;
  state.file = path;
  selectTab(ui.views, $('view-crate'));
  const tab = [...ui.files.querySelectorAll('[role="tab"]')].find((element) => element.dataset.path === path);
  ui.empty.hidden = true;
  ui.lineMark.hidden = true;
  ui.view.hidden = true;
  const editableMain = path === 'src/main.rs';
  ui.codeGrid.hidden = editableMain;
  ui.rustEditorPanel.hidden = !editableMain;
  ui.rustEditorHelp.hidden = !editableMain;
  ui.rustReset.hidden = !editableMain || !state.rustEdited;
  ui.toolbar.hidden = false;
  if (editableMain) {
    ui.rustSource.value = state.rustDraft;
    ui.code.textContent = state.rustDraft;
    renderRustEditor();
  } else renderCode(state.result.files[path], path);
  ui.codeWrap.scrollTo(0, 0);
  if (tab) {
    selectTab(ui.files, tab);
    ui.codeWrap.setAttribute('aria-labelledby', tab.id);
    revealFileTab(tab);
  }
  updateButtons();
}

// Scrolls the row of files so that the selected one shows in full.
function revealFileTab(tab) {
  const list = ui.files;
  if (tab.offsetLeft < list.scrollLeft) list.scrollLeft = tab.offsetLeft;
  else if (tab.offsetLeft + tab.offsetWidth > list.scrollLeft + list.clientWidth) list.scrollLeft = tab.offsetLeft + tab.offsetWidth - list.clientWidth;
  updateFilesOverflow();
}

function updateFilesOverflow() {
  const list = ui.files;
  const hidden = list.scrollWidth - list.clientWidth;
  list.classList.toggle('more-left', list.scrollLeft > 1);
  list.classList.toggle('more-right', list.scrollLeft < hidden - 1);
}

function renderCode(text, path) {
  const rust = path.endsWith('.rs');
  const python = path.endsWith('.py');
  const toml = path.endsWith('.toml');
  const markdown = path.endsWith('.md');
  const lines = text.endsWith('\n') ? text.split('\n').length - 1 : text.split('\n').length;
  ui.gutter.textContent = Array.from({ length: lines }, (_, index) => index + 1).join('\n');
  if (text.length <= HIGHLIGHT_LIMIT && rust) ui.code.replaceChildren(highlightRust(text));
  else if (text.length <= HIGHLIGHT_LIMIT && python) ui.code.replaceChildren(highlightPython(text));
  else if (text.length <= HIGHLIGHT_LIMIT && toml) ui.code.replaceChildren(highlightToml(text));
  else if (text.length <= HIGHLIGHT_LIMIT && markdown) ui.code.replaceChildren(highlightMarkdown(text));
  else ui.code.textContent = text;
  const items = rust ? outline(text) : [];
  ui.jump.hidden = items.length === 0;
  ui.outline.replaceChildren(new Option('Jump to…', ''));
  ui.outline.title = `${plural(items.length, 'item')} in this file`;
  for (const item of items) {
    ui.outline.append(new Option(`${'  '.repeat(item.depth)}${item.kind} ${item.name}`, String(item.line)));
  }
  ui.toolbarNote.textContent = '';
}

function renderRustEditor() {
  const text = ui.rustSource.value;
  if (text.length <= HIGHLIGHT_LIMIT) ui.rustColors.replaceChildren(highlightRust(text));
  else ui.rustColors.textContent = text;
  if (text.endsWith('\n')) ui.rustColors.append(' ');
  const lines = text.split('\n').length;
  if (Number(ui.rustGutter.dataset.lines) !== lines) {
    ui.rustGutter.dataset.lines = String(lines);
    ui.rustGutter.textContent = Array.from({ length: lines }, (_, index) => index + 1).join('\n');
  }
  ui.rustSource.scrollTo(0, 0);
  revealRustCaret();
}

function revealRustCaret() {
  const source = ui.rustSource;
  if (document.activeElement !== source) return;
  const style = getComputedStyle(source);
  if (!rustCharacterWidth) {
    const context = document.createElement('canvas').getContext('2d');
    context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    rustCharacterWidth = context.measureText('0'.repeat(32)).width / 32;
  }
  const end = source.selectionDirection === 'backward' ? source.selectionStart : source.selectionEnd;
  const before = source.value.slice(0, end);
  const lineStart = before.lastIndexOf('\n') + 1;
  const line = before.split('\n').length - 1;
  let column = 0;
  for (const character of before.slice(lineStart)) column = character === '\t' ? column - (column % 4) + 4 : column + 1;
  const height = parseFloat(style.lineHeight);
  const top = parseFloat(style.paddingTop) + line * height;
  if (top < ui.codeWrap.scrollTop) ui.codeWrap.scrollTop = top;
  else if (top + height > ui.codeWrap.scrollTop + ui.codeWrap.clientHeight) ui.codeWrap.scrollTop = top + height - ui.codeWrap.clientHeight;
  const gutter = ui.rustGutter.offsetWidth;
  const left = gutter + parseFloat(style.paddingLeft) + column * rustCharacterWidth;
  if (left < ui.codeWrap.scrollLeft + gutter + parseFloat(style.paddingLeft)) ui.codeWrap.scrollLeft = left - gutter - parseFloat(style.paddingLeft);
  else if (left + rustCharacterWidth > ui.codeWrap.scrollLeft + ui.codeWrap.clientWidth) ui.codeWrap.scrollLeft = left + 2 * rustCharacterWidth - ui.codeWrap.clientWidth;
}

function editRust() {
  state.rustDraft = ui.rustSource.value;
  ui.code.textContent = state.rustDraft;
  state.rustRevision += 1;
  state.rustEdited = state.rustDraft !== (state.result?.files['src/main.rs'] ?? '');
  invalidateActiveRun('src/main.rs changed. Compile and run the updated source.');
  if (state.view === 'crate' && state.file === 'src/main.rs') ui.rustReset.hidden = !state.rustEdited;
  renderRustEditor();
  updateButtons();
}

function resetRust() {
  if (!state.result?.files['src/main.rs']) return;
  if (state.rustEdited && !window.confirm('Discard your edits to src/main.rs and restore the generated starter?')) return;
  state.rustDraft = state.result.files['src/main.rs'];
  state.rustEdited = false;
  state.rustRevision += 1;
  invalidateActiveRun('Rust source reset to the generated starter.');
  if (state.file === 'src/main.rs') {
    ui.rustSource.value = state.rustDraft;
    ui.code.textContent = state.rustDraft;
    renderRustEditor();
  }
  ui.rustReset.hidden = true;
  updateButtons();
}

function isCurrentRun(snapshot, epoch) {
  return epoch === runEpoch && state.result === snapshot.result &&
    state.generationId === snapshot.generationId && state.rustRevision === snapshot.rustRevision &&
    !state.stale && state.rustDraft === snapshot.main;
}

function clearRunOutput() {
  ui.runConsole.hidden = false;
  ui.runProgress.textContent = '';
  ui.compileOutput.textContent = '';
  ui.runStdout.textContent = '';
  ui.runStderr.textContent = '';
  ui.compileOutputSection.hidden = true;
  ui.stdoutSection.hidden = true;
  ui.stderrSection.hidden = true;
  ui.runError.hidden = true;
  ui.runError.textContent = '';
}

function appendRunStream(element, section, value, currentChars) {
  const remaining = Math.max(0, 65_536 - currentChars.chars);
  let end = Math.min(value.length, remaining);
  if (end > 0 && end < value.length) {
    const last = value.charCodeAt(end - 1);
    if (last >= 0xd800 && last <= 0xdbff) end -= 1;
  }
  const chunk = value.slice(0, end);
  element.textContent += chunk;
  currentChars.chars += chunk.length;
  if (chunk.length < value.length) currentChars.truncated = true;
  section.hidden = false;
}

function updateCompilerCost() {
  ui.compilerNotices.hidden = !state.config?.compiler;
  if (!state.config) {
    ui.compilerCost.textContent = 'Browser compiler availability loads with the page configuration.';
    return;
  }
  if (!state.config.compiler) {
    ui.compilerCost.textContent = 'Browser compiler assets are unavailable on this build.';
    return;
  }
  if (state.compilerStarted) {
    ui.compilerCost.textContent = 'VM pauses; released after five idle minutes. Same files may reuse last successful compile; edits reuse it while warm. Each run starts fresh module state. Runs locally; desktop-class memory needed.';
    return;
  }
  const bytes = Number(state.config.compiler_download_bytes);
  const size = Number.isFinite(bytes) && bytes > 0
    ? `about ${(bytes / (1024 * 1024)).toFixed(0)} MiB of compiler assets`
    : 'compiler assets';
  ui.compilerCost.textContent = `First run downloads ${size} and starts a 1 GiB Linux VM. Identical files may reuse compiled code; edits reuse the warm VM. Pauses between runs; expires after five idle minutes. Rust standard library and PyO3 only; runs locally. Desktop-class memory needed.`;
}

function invalidateActiveRun(reason) {
  const phase = state.runPhase;
  const wasRunning = state.runBusy;
  const hadRunUi = !ui.runConsole.hidden;
  if (wasRunning) {
    ++runEpoch;
    state.runBusy = false;
    state.runPhase = null;
    clearTimeout(runTimeout);
    if (phase === 'compiling') browserCompiler?.cancel();
  }
  state.resetRequired = state.rustLoadAttempts >= 8;
  if (wasRunning || hadRunUi) {
    clearRunOutput();
    ui.runProgress.textContent = wasRunning ? 'Run result is out of date.' : '';
    ui.runError.hidden = false;
    ui.runError.textContent = wasRunning ? `${reason} Previous run was cancelled.` : reason;
  }
  if (phase === 'running') void recoverPythonWorker(`${reason} Python restarted.`);
  updateButtons();
}

async function compileAndRun() {
  if (ui.compileRun.disabled || !state.result || state.stale || !state.config?.compiler) return;
  clearTimeout(liveTimer);
  liveTimer = null;
  livePending = false;
  const main = state.rustDraft;
  const snapshot = {
    result: state.result,
    generationId: state.generationId,
    rustRevision: state.rustRevision,
    main,
    files: { ...state.result.files, 'src/main.rs': main },
  };
  const epoch = ++runEpoch;
  state.runBusy = true;
  state.resetRequired = false;
  state.runPhase = 'compiling';
  clearRunOutput();
  ui.cancelRun.hidden = false;
  ui.runProgress.textContent = 'Starting browser compiler…';
  showFile('src/main.rs');
  updateButtons();
  let compiled;
  try {
    if (!state.ready) throw new Error('Python is not ready. Retry Python startup before compiling and running.');
    browserCompiler ??= new BrowserCompiler(new URL(state.config.compiler, document.baseURI).href);
    const compilePromise = browserCompiler.compile(snapshot.files, (progress) => {
      if (epoch !== runEpoch || !state.runBusy) return;
      if (!state.compilerStarted && (progress.phase === 'compiling' || progress.phase === 'cached')) {
        state.compilerStarted = true;
        updateCompilerCost();
      }
      ui.runProgress.textContent = progress.text;
    });
    compiled = await Promise.race([
      compilePromise,
      new Promise((_, reject) => {
        runTimeout = setTimeout(() => {
          browserCompiler?.cancel();
          reject(new Error('Compilation timed out. Retry after checking your connection and available memory.'));
        }, 5 * 60_000);
      }),
    ]);
    clearTimeout(runTimeout);
    if (!isCurrentRun(snapshot, epoch)) return;
    if (compiled.diagnostics) {
      ui.compileOutput.textContent = compiled.diagnostics;
      ui.compileOutputSection.hidden = false;
    }
    state.runPhase = 'running';
    ui.runProgress.textContent = 'Running the compiled Rust program in the current Python session…';
    const streamChars = { stdout: { chars: 0, truncated: false }, stderr: { chars: 0, truncated: false } };
    const streamed = { stdout: false, stderr: false };
    state.rustLoadAttempts += 1;
    const outcome = await Promise.race([
      call('runRust', { moduleName: compiled.moduleName, wasm: compiled.wasm }, {
        onStream: ({ stream, text }) => {
          if (!isCurrentRun(snapshot, epoch)) return;
          const stdout = stream === 'stdout';
          streamed[stdout ? 'stdout' : 'stderr'] = true;
          appendRunStream(stdout ? ui.runStdout : ui.runStderr, stdout ? ui.stdoutSection : ui.stderrSection, text, streamChars[stdout ? 'stdout' : 'stderr']);
        },
      }),
      new Promise((_, reject) => {
        runTimeout = setTimeout(() => reject(Object.assign(new Error('Program exceeded the 30 second execution limit.'), { resetRequired: true })), 30_000);
      }),
    ]);
    clearTimeout(runTimeout);
    if (!isCurrentRun(snapshot, epoch)) return;
    const result = outcome.result;
    if (result.stdout && !streamed.stdout) appendRunStream(ui.runStdout, ui.stdoutSection, result.stdout, streamChars.stdout);
    if (result.stderr && !streamed.stderr) appendRunStream(ui.runStderr, ui.stderrSection, result.stderr, streamChars.stderr);
    const truncated = result.truncated || streamChars.stdout.truncated || streamChars.stderr.truncated;
    ui.runProgress.textContent = result.success ? 'Program finished successfully.' : 'Program failed.';
    if (truncated) ui.runProgress.textContent += ' Output truncated (65,536 characters per stream).';
    const loadLimitReached = state.rustLoadAttempts >= 8;
    state.resetRequired = loadLimitReached;
    if (loadLimitReached && result.success) ui.runProgress.textContent += ' Restart Python before the next run.';
    if (!result.success) {
      ui.runError.hidden = false;
      ui.runError.textContent = result.error || 'The Rust program failed.';
      if (result.resetRequired && /load limit reached/i.test(result.error ?? '')) {
        state.resetRequired = true;
        ui.runProgress.textContent = 'Run limit reached. Restart Python before compiling another program.';
      } else if (result.resetRequired) {
        state.resetRequired = false;
        await recoverPythonWorker('Python restarted because Rust execution left the runtime in an unsafe state.');
      }
    }
  } catch (error) {
    clearTimeout(runTimeout);
    if (epoch !== runEpoch || error.cancelled) return;
    if (error.diagnostics) {
      ui.compileOutput.textContent = error.diagnostics;
      ui.compileOutputSection.hidden = false;
    } else if (state.runPhase === 'compiling') {
      ui.compileOutput.textContent = error.message;
      ui.compileOutputSection.hidden = false;
    } else {
      ui.runError.hidden = false;
      ui.runError.textContent = error.message;
    }
    ui.runProgress.textContent = state.runPhase === 'compiling' ? 'Compilation failed. Edit the source and retry.' : 'Execution failed.';
    if (state.runPhase === 'running' || error.resetRequired) {
      state.resetRequired = false;
      await recoverPythonWorker('Python restarted after execution stopped.');
    }
  } finally {
    if (epoch === runEpoch) {
      clearTimeout(runTimeout);
      state.runBusy = false;
      state.runPhase = null;
      ui.cancelRun.hidden = true;
      updateButtons();
    }
  }
}

async function recoverPythonWorker(note) {
  clearTimeout(runTimeout);
  packagesLost ||= Boolean(state.installed);
  const lostPackages = packagesLost;
  state.installed = null;
  if (lostPackages && state.result) markStale(true);
  ui.installed.hidden = true;
  state.ready = false;
  state.rustLoadAttempts = 0;
  stopWorker(Object.assign(new Error('Python worker restarted after Rust execution.'), { cancelled: true }));
  showNote(`${note}${lostPackages ? ' Installed packages were cleared; reinstall them before generating package bindings.' : ''}`);
  try {
    await boot();
    packagesLost = false;
  } catch (error) {
    setStatus('error', 'Python failed to restart');
    showMessage(`Python could not restart: ${error.message} Retry to start Python again.`);
    showRetry(true);
  }
  updateButtons();
}

function cancelCompileOrRun() {
  if (!state.runBusy) return;
  if (state.runPhase === 'compiling') {
    ++runEpoch;
    browserCompiler?.cancel();
    clearTimeout(runTimeout);
    state.runBusy = false;
    state.runPhase = null;
    ui.cancelRun.hidden = true;
    ui.runProgress.textContent = 'Compilation cancelled. Python and installed packages are unchanged.';
    updateButtons();
    return;
  }
  ++runEpoch;
  state.runBusy = false;
  state.runPhase = null;
  ui.cancelRun.hidden = true;
  ui.runProgress.textContent = 'Execution cancelled. Restarting Python…';
  void recoverPythonWorker('Python restarted after execution was cancelled.');
  updateButtons();
}

function restartPython() {
  if (!state.resetRequired) return;
  state.resetRequired = false;
  void recoverPythonWorker('Python restarted at your request.');
  updateButtons();
}

// Generated helpers that every class or enum has; the outline lists the API.
const BOILERPLATE = new Set([
  'from_py_any', 'bind', 'as_any', 'clone_ref', 'into_py_any', 'extract', 'into_pyobject',
  'as_py_name', 'from_py_name', 'pyo3_embed_python_source_code',
]);
const ITEM = /^( *)(?:pub )?(mod|struct|enum|trait|fn|type|const) (?:r#)?([A-Za-z_]\w*)/;
const IMPL = /^( *)impl\b(.*)\{$/;

// The items of generated Rust with their line numbers, skipping trait
// implementations, whose methods the trait declarations already list.
function outline(text) {
  const items = [];
  let skipIndent = null;
  for (const [index, line] of text.split('\n').entries()) {
    if (skipIndent !== null) {
      if (line === `${skipIndent}}`) skipIndent = null;
      continue;
    }
    const impl = IMPL.exec(line);
    if (impl && / for /.test(impl[2])) {
      skipIndent = impl[1];
      continue;
    }
    const match = ITEM.exec(line);
    if (!match || BOILERPLATE.has(match[3]) || /^_/.test(match[3])) continue;
    if (match[2] === 'const' && /^PYO3_BINDGEN_/.test(match[3])) continue;
    items.push({ line: index + 1, depth: match[1].length / 4, kind: match[2], name: match[3] });
  }
  return items;
}

// Shows a line of bindings.rs, with a way back to the view that led there.
function showBindingsLine(line, label, from) {
  const previousFile = state.file;
  showFile(DEFAULT_FILE);
  jumpToLine(line);
  const back = document.createElement('button');
  back.type = 'button';
  back.className = 'link';
  back.textContent = `Back to ${$(`view-${from}`).textContent.toLowerCase()}`;
  back.title = `${label}, line ${line}`;
  back.addEventListener('click', () => {
    if (from === 'crate') showFile(previousFile);
    else showView(from);
    const tab = from === 'crate'
      ? [...ui.files.querySelectorAll('[role="tab"]')].find((element) => element.dataset.path === previousFile)
      : $(`view-${from}`);
    tab?.focus();
  });
  ui.toolbarNote.replaceChildren(back);
}

function jumpToLine(line) {
  const style = getComputedStyle(ui.code.parentElement);
  const height = parseFloat(style.lineHeight);
  const top = parseFloat(style.paddingTop) + (line - 1) * height;
  ui.codeWrap.scrollTo({ top: Math.max(0, top - ui.codeWrap.clientHeight / 3), behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
  ui.lineMark.style.top = `${top}px`;
  ui.lineMark.style.height = `${height}px`;
  ui.lineMark.hidden = false;
  ui.lineMark.classList.remove('flash');
  void ui.lineMark.offsetWidth;
  ui.lineMark.classList.add('flash');
}

/* ---------- Overview ---------- */

function bindingsModel() {
  const result = state.result;
  result.overview ??= parseBindings(result.files[DEFAULT_FILE]);
  return result.overview;
}

function overviewView() {
  const wrap = element('div', 'overview-view');
  const modules = bindingsModel();
  // Rows point at their Python while the editor shows the code they came from.
  const { source, module } = state.result;
  const linked = source !== null && source === ui.source.value && state.mode === 'source';
  const point = linked ? ' Point at one to find its Python on the left; select it to see its Rust in bindings.rs.' : ' Select one to see its code in bindings.rs.';
  wrap.append(element('p', 'view-intro', `Each Python item beside the Rust that calls it.${point}`));
  wrap.append(
    renderOverview(modules, state.result.python_api ?? {}, {
      file: 'bindings.rs',
      onJump: (line) => showBindingsLine(line, 'bindings.rs', 'overview'),
      sources: linked ? pythonLines(source) : null,
      sourceModule: module,
      onSource: (line) => markSourceLine(line, ui.sourceFocus),
    }),
  );
  return wrap;
}

/* ---------- Type map ---------- */

// Rust types as the analysis spells them, such as `& :: pyo3 :: Bound < 'py , T >`,
// shortened for reading: `&Bound<'py, T>`.
function rustType(spelling) {
  return spelling
    .replace(/\s*(::|<|>|\(|\)|\[|\]|;)\s*/g, '$1')
    .replace(/\s*,\s*/g, ', ')
    .replace(/&\s*/g, '&')
    .replace(/'\s+/g, "'")
    .replace(/(?:::)?(?:std|pyo3|core|alloc)(?:::[a-z_]+)*::([A-Za-z_]\w*)/g, '$1')
    .replace(/<(\s*)>/g, '<>')
    .replace(/\((\w+(?:<[^()]*>)?), \)/g, '($1,)');
}

const ORIGINS = {
  annotation: 'annotation',
  class_declaration: 'class annotation',
  runtime_value: 'value',
  getter: 'getter type',
  unknown: 'unknown',
  async_handle: 'async',
  structural: 'by construction',
};

function positionLabel(identity, mapping) {
  const position = identity.position;
  switch (position.kind) {
    case 'parameter':
      return mapping.python_parameter ?? `argument ${position.ordinal}`;
    case 'return':
      return 'returns';
    case 'getter':
      return 'get';
    case 'setter':
      return 'set';
    default:
      return position.kind.replace(/_/g, ' ');
  }
}

// The annotation that decided the type: for a dataclass field or another class
// annotation, the class's declaration rather than the accessor's own.
function annotationText(mapping) {
  const annotation = mapping.annotation;
  const spelling = mapping.effective_annotation?.spelling ?? annotation.spelling;
  if (spelling) return spelling;
  if (mapping.origin === 'runtime_value') return 'from its value';
  if (mapping.origin === 'class_declaration') return 'class annotation';
  if (annotation.status === 'missing') return 'not annotated';
  return '';
}

// Reasons that describe a deliberate type rather than lost type information;
// the others mark a fallback to a less specific type, as Reason::is_fallback does.
const EXACT_REASONS = new Set(['mapped', 'explicit_any', 'constructor_self', 'setter_unit']);

function reasons(mapping) {
  const found = new Set();
  for (const component of mapping.components ?? []) {
    const list = mapping.direction === 'owned' ? component.owned_reasons : component.parameter_reasons;
    for (const reason of list ?? []) if (reason !== 'mapped') found.add(reason);
  }
  return [...found].map((code) => ({ text: code.replace(/_/g, ' '), fallback: !EXACT_REASONS.has(code) }));
}

function analysis() {
  const result = state.result;
  if (result.parsedAnalysis === undefined) {
    try {
      result.parsedAnalysis = result.analysis ? JSON.parse(result.analysis) : null;
    } catch {
      result.parsedAnalysis = null;
    }
  }
  return result.parsedAnalysis;
}

function typeRows() {
  const rows = [];
  for (const position of analysis()?.positions ?? []) {
    const { identity } = position;
    if (identity.position.kind === 'setter_result') continue;
    for (const mapping of position.mappings) {
      rows.push({
        symbol: identity.symbol,
        kind: identity.symbol_kind,
        position: positionLabel(identity, mapping),
        python: annotationText(mapping),
        rust: rustType(mapping.emitted_type),
        fullRust: mapping.emitted_type,
        origin: ORIGINS[mapping.origin] ?? mapping.origin,
        reasons: reasons(mapping),
        location: mapping.locations?.[0],
      });
    }
  }
  return rows;
}

function element(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.append(...children.filter((child) => child !== null && child !== undefined));
  return node;
}

function typesView() {
  const rows = typeRows();
  const wrap = element('div', 'types');
  if (rows.length === 0) {
    wrap.append(element('p', 'view-intro', 'No typed positions: the module has no functions, methods or properties to map.'));
    return wrap;
  }
  const fallbacks = rows.filter((row) => row.reasons.some((reason) => reason.fallback)).length;
  const summary =
    fallbacks === 0
      ? rows.length === 1
        ? 'It maps to a specific Rust type.'
        : `All ${rows.length} map to specific Rust types.`
      : `${fallbacks} of ${rows.length} ${fallbacks === 1 ? 'falls' : 'fall'} back to a more general type, for the reason marked.`;
  wrap.append(element('p', 'view-intro', `How each Python type became a Rust type. ${summary} Select a Rust type to find it in bindings.rs.`));
  const table = element('table', 'type-table');
  const head = element('thead', null, element('tr', null, element('th', null, 'Python'), element('th', null, 'Rust'), element('th', null, 'From')));
  const body = element('tbody');
  let symbol = null;
  for (const row of rows) {
    if (row.symbol !== symbol) {
      symbol = row.symbol;
      const cell = element('th', null, element('code', null, row.symbol), ' ', element('span', 'kind', row.kind));
      cell.colSpan = 3;
      cell.scope = 'rowgroup';
      body.append(element('tr', 'symbol', cell));
    }
    const python = element('td', null, element('span', 'position', row.position), row.python ? element('code', null, row.python) : null);
    const jump = element('button', 'rust-type', row.rust);
    jump.type = 'button';
    jump.title = rustType(row.fullRust).length > row.rust.length ? rustType(row.fullRust) : 'Find in bindings.rs';
    jump.addEventListener('click', () => jumpToMember(row.location));
    const from = element('td', null, element('span', 'origin', row.origin), ...row.reasons.map((reason) => element('span', reason.fallback ? 'reason fallback' : 'reason', reason.text)));
    body.append(element('tr', null, python, element('td', null, jump), from));
  }
  table.append(head, body);
  wrap.append(table);
  return wrap;
}

// Finds the generated member of an analysis location in bindings.rs.
function jumpToMember(location) {
  if (!location) return;
  const lines = state.result.files[DEFAULT_FILE].split('\n');
  const scope = location.scope ?? {};
  const owner = scope.class ?? scope.name ?? null;
  let start = 0;
  if (owner) {
    const header = new RegExp(`^ *(?:pub )?(?:trait ${owner}\\b|impl(?:<[^>]*>)? ${owner} \\{|impl\\b.* for ${owner} \\{|struct ${owner}\\b)`);
    const preferred = scope.kind === 'trait' ? new RegExp(`^ *pub trait ${owner}\\b`) : scope.kind === 'inherent' ? new RegExp(`^ *impl(?:<[^>]*>)? ${owner} \\{`) : header;
    start = Math.max(0, lines.findIndex((line) => preferred.test(line)));
  }
  const member = new RegExp(`^ *(?:pub )?fn (?:r#)?${location.member}\\b`);
  let index = lines.findIndex((line, at) => at >= start && member.test(line));
  if (index < 0) index = lines.findIndex((line) => member.test(line));
  if (index < 0) return;
  showBindingsLine(index + 1, location.member, 'types');
}

function typesText() {
  return typeRows()
    .map((row) => [row.symbol, row.position, row.python, row.rust, [row.origin, ...row.reasons.map((reason) => reason.text)].join(', ')].join('\t'))
    .join('\n');
}

/* ---------- Report ---------- */

function definitions(pairs) {
  const list = element('dl', 'facts');
  for (const [term, value] of pairs) list.append(element('dt', null, term), element('dd', null, value));
  return list;
}

function reportView() {
  const { report } = state.result;
  const wrap = element('div', 'report');
  const python = report.python ? `${report.python.implementation} ${report.python.version.split(' ')[0]}` : 'unknown';
  wrap.append(
    definitions([
      ['Generator', `pyo3_bindgen ${report.generator.version}`],
      ['PyO3', report.generator.pyo3_version ?? 'unknown'],
      ['Python', python],
      ['Complete', report.complete ? 'yes' : 'no: some submodules or attributes could not be read'],
    ]),
  );
  // The table leaves out the kinds of item that no module has, beyond the common three.
  const counts = ['classes', 'functions', 'properties', 'enums', 'type_vars', 'submodules', 'imports'].filter(
    (key, index) => index < 3 || report.modules.some((module) => module.counts[key]),
  );
  const table = element('table', 'module-table');
  table.append(
    element('thead', null, element('tr', null, element('th', null, 'Module'), ...counts.map((key) => element('th', null, key.replace('_', ' '))))),
  );
  const body = element('tbody');
  for (const module of report.modules) {
    body.append(element('tr', null, element('td', null, element('code', null, module.name)), ...counts.map((key) => element('td', null, String(module.counts[key] ?? 0)))));
  }
  table.append(body);
  wrap.append(element('h3', null, 'Modules'), element('div', 'table-scroll', table));

  const enums = report.modules.flatMap((module) => (module.enums ?? []).map((item) => ({ module: module.name, ...item })));
  if (enums.length) {
    const list = element('ul', 'enums');
    for (const item of enums) {
      const variants = (item.variants ?? []).map((variant) => `${variant.python_name} → ${variant.rust_name}`).join(', ');
      list.append(element('li', null, element('code', null, item.python_path ?? item.name ?? ''), ` ${variants}`));
    }
    wrap.append(element('h3', null, 'Enums'), list);
  }

  const diagnostics = report.diagnostics ?? [];
  const warnings = diagnostics.filter((diagnostic) => diagnostic.severity !== 'info');
  const info = diagnostics.filter((diagnostic) => diagnostic.severity === 'info');
  wrap.append(element('h3', 'warnings-title', warnings.length ? plural(warnings.length, 'warning') : 'No warnings'));
  if (warnings.length) wrap.append(diagnosticList(warnings));
  if (info.length) {
    const details = element('details', 'info', element('summary', null, `${plural(info.length, 'discovery note')}`), diagnosticList(info));
    wrap.append(details);
  }
  return wrap;
}

const DIAGNOSTICS_SHOWN = 200;

function diagnosticList(diagnostics) {
  const list = element('ul', 'diagnostics');
  for (const diagnostic of diagnostics.slice(0, DIAGNOSTICS_SHOWN)) {
    list.append(
      element('li', null, element('code', 'code', diagnostic.code), diagnostic.path ? element('code', 'path', diagnostic.path) : null, ` ${diagnostic.message}`),
    );
  }
  if (diagnostics.length > DIAGNOSTICS_SHOWN) list.append(element('li', null, `… and ${diagnostics.length - DIAGNOSTICS_SHOWN} more`));
  return list;
}

/* ---------- Stats ---------- */

const plural = (count, one, many = `${one}s`) => `${count.toLocaleString('en')} ${count === 1 ? one : many}`;

function renderStats(result, ms) {
  const totals = { classes: 0, functions: 0, properties: 0, enums: 0, submodules: 0 };
  for (const module of result.report.modules) {
    for (const key of Object.keys(totals)) totals[key] += module.counts[key] ?? 0;
  }
  const code = result.files[DEFAULT_FILE];
  const lines = code.split('\n').length;
  const parts = [
    plural(totals.functions, 'function'),
    plural(totals.classes, 'class', 'classes'),
    plural(totals.properties, 'property', 'properties'),
  ];
  if (totals.enums) parts.push(plural(totals.enums, 'enum'));
  if (totals.submodules) parts.push(plural(totals.submodules, 'submodule'));
  parts.push(`${plural(lines, 'line')} of Rust`, `${Math.round(ms).toLocaleString('en')} ms`);
  ui.stats.textContent = parts.join(' · ');
  if (code.length > HIGHLIGHT_LIMIT) {
    showNote(`This output has ${(code.length / 1e6).toFixed(1)} MB, so it is shown without colours.`);
  }
  const warnings = (result.report.diagnostics ?? []).filter((diagnostic) => diagnostic.severity !== 'info').length;
  const labels = [];
  if (warnings) labels.push(plural(warnings, 'warning'));
  if (!result.report.complete) labels.push('incomplete');
  ui.warnings.hidden = labels.length === 0;
  ui.warnings.textContent = labels.join(' · ');
}

/* ---------- Actions ---------- */

function selectedModules(mode) {
  if (mode === 'package') {
    return [...ui.installedModules.querySelectorAll('input:checked')].map((box) => box.value);
  }
  return ui.modules.value.split(/[\s,]+/).filter(Boolean);
}

async function run(task, busyText) {
  if (!state.ready || state.busy || state.runBusy) return null;
  const epoch = ++actionEpoch;
  state.busy = true;
  updateButtons();
  updateExampleGuide();
  setStatus('busy', busyText);
  showMessage('');
  try {
    return await task();
  } catch (error) {
    if (error.cancelled) return null;
    showMessage(error.message);
    return null;
  } finally {
    if (epoch !== actionEpoch) return;
    state.busy = false;
    if (state.ready) setStatus('ready', ui.status.dataset.readyText ?? 'Ready');
    updateButtons();
    updateExampleGuide();
    const queuedExample = pendingExampleGeneration;
    pendingExampleGeneration = null;
    if (queuedExample && state.ready && queuedExample.selection === exampleSelectionGeneration && queuedExample.editRevision === userEditRevision) {
      clearTimeout(liveTimer);
      livePending = false;
      generate();
    } else if (livePending) {
      scheduleLive(0);
    }
  }
}

let liveTimer = null;
let livePending = false;

// Live generation follows edits of pasted code and option changes; a change
// during a run generates again once that run ends.
function scheduleLive(delay = LIVE_DELAY) {
  clearTimeout(liveTimer);
  liveTimer = null;
  livePending = false;
  if (!ui.live.checked || !state.ready) {
    updateExampleGuide();
    return;
  }
  if (state.mode === 'package' && !state.installed) {
    updateExampleGuide();
    return;
  }
  liveTimer = setTimeout(() => {
    liveTimer = null;
    if (state.busy) livePending = true;
    else generate({ live: true });
  }, delay);
  updateExampleGuide();
}

async function generate({ live = false } = {}) {
  if (!live) {
    clearTimeout(liveTimer);
    liveTimer = null;
  }
  const mode = state.mode;
  if (mode === 'source') {
    const valid = live ? ui.sourceModule.checkValidity() : ui.sourceModule.reportValidity();
    if (!valid) return;
  }
  if (mode === 'package' && !state.installed) {
    if (live) return;
    await install();
    if (!state.installed) return;
  }
  // The inputs stay editable during a run; the link records what was generated.
  const input = { source: ui.source.value, requirement: ui.requirement.value.trim(), example: matchingExample(), options: readOptions() };
  const modules = mode === 'source' ? [ui.sourceModule.value.trim()] : selectedModules(mode);
  if (mode !== 'source' && modules.length === 0) {
    if (!live) showMessage(mode === 'package' ? 'Select at least one module of the package.' : 'Name at least one module, such as math.');
    return;
  }
  const options = { ...input.options, analysis: true };
  const mainBody = mode === 'source'
    ? selectStarterBody(input.source, modules[0], options, state.defaults, state.examples.get(modules[0]))
    : null;
  const outcome = await run(
    () =>
      mode === 'source'
        ? call('generateSource', { source: input.source, module: modules[0], options, mainBody })
        : call('generateModules', { modules, options }),
    `Generating bindings for ${modules.join(', ')}…`,
  );
  state.generated = true;
  if (!outcome) {
    if (mode === 'source') pointToSourceLine();
    // A live run keeps the last bindings, marked as outdated, while the code has an error.
    if (live && state.result && !ui.message.hidden) markStale(true);
    else if (!ui.message.hidden) clearOutput();
    return;
  }
  const matchesCurrentInput =
    state.mode === mode &&
    same(readOptions(), input.options) &&
    (mode === 'source'
      ? ui.source.value === input.source && ui.sourceModule.value.trim() === modules[0]
      : same(selectedModules(mode), modules) && (mode !== 'package' || ui.requirement.value.trim() === input.requirement));
  ui.sourceError.hidden = true;
  state.result = { ...outcome.result, name: crateName(modules), source: mode === 'source' ? input.source : null, module: modules[0] };
  state.generationId += 1;
  if (!state.rustEdited) state.rustDraft = state.result.files['src/main.rs'] ?? '';
  state.rustEdited = state.rustDraft !== (state.result.files['src/main.rs'] ?? '');
  state.resultOptions = input.options;
  markStale(!matchesCurrentInput);
  if (!live) showNote('');
  renderTabs();
  renderStats(outcome.result, outcome.ms);
  updateButtons();
  if (matchesCurrentInput) await saveToHash(mode, modules, input);
}

// Python reports an error in the pasted code at `__init__.py`, the file that
// the code becomes; the message names only the line, with a button that selects it.
function pointToSourceLine() {
  const text = ui.message.textContent;
  const match = /\(__init__\.py, line (\d+)\)/.exec(text);
  if (!match) return;
  const line = Number(match[1]);
  ui.message.textContent = text.replace(match[0], `(line ${line})`);
  if (line >= 1 && line <= ui.source.value.split('\n').length) markSourceLine(line);
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'link';
  button.textContent = `Show line ${line}`;
  button.addEventListener('click', () => selectSourceLine(line));
  ui.message.append(' ', button);
}

function selectSourceLine(line) {
  const lines = ui.source.value.split('\n');
  if (line < 1 || line > lines.length) return;
  const start = lines.slice(0, line - 1).reduce((sum, text) => sum + text.length + 1, 0);
  ui.source.focus({ preventScroll: true });
  ui.source.setSelectionRange(start, start + lines[line - 1].length);
  const height = parseFloat(getComputedStyle(ui.source).lineHeight);
  ui.editor.scrollTop = Math.max(0, (line - 1) * height - ui.editor.clientHeight / 3);
}

function markStale(stale) {
  state.stale = stale;
  ui.stale.hidden = !stale;
  ui.codeWrap.classList.toggle('is-stale', stale);
  updateButtons();
  updateExampleGuide();
}

async function install() {
  const requirement = ui.requirement.value.trim();
  if (!requirement) {
    showMessage('Enter a package name from PyPI, such as attrs.');
    ui.requirement.focus();
    return;
  }
  state.installed = null;
  ui.installed.hidden = true;
  const outcome = await run(() => call('install', { requirement }), `Installing ${requirement}…`);
  // Editing the package name during the installation discards its result.
  if (!outcome || ui.requirement.value.trim() !== requirement) return;
  const { name, version, modules } = outcome.result;
  state.installed = { name, version, modules };
  ui.installedTitle.textContent = `${name} ${version}: modules to bind`;
  ui.installedModules.replaceChildren();
  for (const [index, module] of modules.entries()) {
    const label = document.createElement('label');
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.value = module;
    box.checked = index === 0;
    label.append(box, ` ${module}`);
    ui.installedModules.append(label);
  }
  if (modules.length === 0) {
    ui.installedModules.textContent = 'This package has no importable top-level module.';
  }
  ui.installed.hidden = false;
  showNote(`Installed ${name} ${version} with ${plural(modules.length, 'importable module')}.`);
}

function crateName(modules) {
  const root = (modules[0] ?? 'python').split('.')[0].toLowerCase().replace(/[^a-z0-9_]/g, '_');
  return `${root.replace(/^_+|_+$/g, '') || 'python'}_bindings`;
}

function currentText() {
  if (!state.result) return undefined;
  if (state.view === 'overview') return overviewText(bindingsModel(), state.result.python_api ?? {});
  if (state.view === 'types') return typesText();
  if (state.view === 'report') return `${JSON.stringify(state.result.report, null, 2)}\n`;
  return state.file === 'src/main.rs' ? state.rustDraft : state.result.files[state.file];
}

async function copy() {
  const text = currentText();
  if (text === undefined) return;
  try {
    await navigator.clipboard.writeText(text);
    flash(ui.copy, 'Copied');
  } catch {
    if (state.view === 'crate' && state.file === 'src/main.rs') {
      ui.rustSource.focus();
      ui.rustSource.select();
      flash(ui.copy, 'Press Ctrl+C');
      return;
    }
    const range = document.createRange();
    range.selectNodeContents(state.view === 'crate' ? ui.code : ui.view);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    flash(ui.copy, 'Press Ctrl+C');
  }
}

function flash(button, text) {
  const original = button.textContent;
  button.textContent = text;
  clearTimeout(button.flashTimer);
  button.flashTimer = setTimeout(() => {
    if (button === ui.copy) updateButtons();
    else button.textContent = original;
  }, 1500);
}

async function download() {
  const result = state.result;
  if (!result || state.stale) return;
  const revision = state.rustRevision;
  const files = { ...result.files, 'src/main.rs': state.rustDraft };
  const { name } = result;
  const outcome = await run(() => call('crateZip', { files, name }), 'Packing the crate…');
  if (!outcome) return;
  if (state.result !== result || state.stale || state.rustRevision !== revision || files['src/main.rs'] !== state.rustDraft) {
    showNote('The Rust source changed while the crate was being packed. Download the current source again.');
    return;
  }
  const url = URL.createObjectURL(new Blob([outcome.result], { type: 'application/zip' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = `${name}.zip`;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

async function cancel() {
  if (!state.busy) return;
  ++actionEpoch;
  clearTimeout(liveTimer);
  livePending = false;
  stopWorker(Object.assign(new Error('Cancelled'), { cancelled: true }));
  state.busy = false;
  state.ready = false;
  packagesLost ||= Boolean(state.installed);
  state.installed = null;
  ui.installed.hidden = true;
  markStale(Boolean(state.result));
  showNote('Cancelled. Python restarted, so packages installed earlier need installing again.');
  try {
    await boot();
    if (packagesLost) {
      showNote('Python restarted. Install the package again before generating package bindings.');
      packagesLost = false;
    }
  } catch (error) {
    setStatus('error', 'Python failed to start');
    showMessage(`Python could not restart: ${error.message} Retry to try loading Pyodide, the generator, and Python glue again.`);
    showRetry(true);
    updateButtons();
    notifyHost('wasm-demo-error', { ...(state.config?.build ? { build: state.config.build } : {}), message: error.message });
  }
}

/* ---------- Shareable state in the URL hash ---------- */

function toBase64Url(bytes) {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function fromBase64Url(text) {
  const binary = atob(text.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

async function transform(bytes, stream) {
  return new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(stream)).arrayBuffer());
}

function sourceLineCount(text) {
  let lines = 1;
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === '\r') {
      lines += 1;
      if (text[index + 1] === '\n') index += 1;
    } else if (text[index] === '\n') {
      lines += 1;
    }
  }
  return lines;
}

function encodeShareSource(text) {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length > LINK_SOURCE_BYTE_LIMIT || sourceLineCount(text) > LINK_SOURCE_LINE_LIMIT) {
    throw new Error('Source exceeds the shared-link size limits');
  }
  return bytes;
}

async function readBounded(stream) {
  const reader = stream.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (size + value.byteLength > LINK_SOURCE_BYTE_LIMIT) {
        await reader.cancel();
        throw new Error('Shared source exceeds the decoded size limit');
      }
      chunks.push(value);
      size += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

// Pasted code travels compressed in `z`; links from before compression use `code`.
async function encodeCode(text) {
  const bytes = encodeShareSource(text);
  return toBase64Url(await transform(bytes, new CompressionStream('deflate-raw')));
}

async function decodeCode(params) {
  const encoded = params.has('z') ? params.get('z') : params.get('code');
  if (encoded.length > LINK_CODE_LIMIT) throw new Error('Shared source exceeds the encoded size limit');
  let bytes;
  if (params.has('z')) {
    bytes = await readBounded(new Blob([fromBase64Url(encoded)]).stream().pipeThrough(new DecompressionStream('deflate-raw')));
  } else {
    bytes = fromBase64Url(encoded);
    if (bytes.byteLength > LINK_SOURCE_BYTE_LIMIT) throw new Error('Shared source exceeds the decoded size limit');
  }
  const source = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  if (sourceLineCount(source) > LINK_SOURCE_LINE_LIMIT) throw new Error('Shared source exceeds the line limit');
  return source;
}

async function saveToHash(mode, modules, input) {
  const editRevision = userEditRevision;
  const generationId = state.generationId;
  const isCurrent = () => editRevision === userEditRevision && generationId === state.generationId && mode === state.mode;
  const params = new URLSearchParams();
  if (mode === 'source') {
    if (input.example) {
      params.set('example', input.example);
    } else {
      let code;
      try {
        code = await encodeCode(input.source);
      } catch (error) {
        if (!isCurrent()) return;
        if (error.message !== 'Source exceeds the shared-link size limits') throw error;
        showNote('This code is too long to keep in the page link, so the link does not include it.');
        history.replaceState(null, '', location.pathname + location.search);
        return;
      }
      if (!isCurrent()) return;
      if (code.length > LINK_CODE_LIMIT) {
        showNote('This code is too long to keep in the page link, so the link does not include it.');
        history.replaceState(null, '', location.pathname + location.search);
        return;
      }
      params.set('name', modules[0]);
      params.set('z', code);
    }
  } else if (mode === 'package') {
    params.set('package', input.requirement);
    params.set('modules', modules.join(' '));
  } else {
    params.set('modules', modules.join(' '));
  }
  if (!isCurrent()) return;
  optionsToParams(params, input.options);
  history.replaceState(null, '', `#${params}`);
}

// Restores the inputs from the link; options wait for the generator's defaults.
async function loadFromHash() {
  const params = new URLSearchParams(location.hash.slice(1));
  const restored = { mode: 'source', params };
  try {
    if (params.has('package')) {
      selectMode('package');
      ui.requirement.value = params.get('package');
      return { ...restored, mode: 'package', modules: (params.get('modules') ?? '').split(/\s+/).filter(Boolean) };
    }
    if (params.has('example') && params.get('example') in EXAMPLES) {
      await loadExample(params.get('example'));
      return restored;
    }
    if (params.has('z') || params.has('code')) {
      const source = await decodeCode(params);
      selectMode('source');
      setSource(source);
      ui.sourceModule.value = params.get('name') ?? 'module';
      ui.example.value = '';
      state.guideExample = await matchingExperiment(source, ui.sourceModule.value.trim());
      updateExampleGuide();
      return restored;
    }
    if (params.has('modules')) {
      selectMode('module');
      ui.modules.value = params.get('modules');
      return { ...restored, mode: 'module' };
    }
  } catch {
    showNote('The link holds code that could not be read, so the example is shown instead.');
  }
  await loadExample(DEFAULT_EXAMPLE).catch((error) => showMessage(`The example could not load (${error.message}).`));
  return restored;
}

/* ---------- Wiring ---------- */

function wire() {
  setupTabs($('tab-source').parentElement, (tab) => {
    selectMode(tab.id.slice('tab-'.length));
    markUserEdit();
  });
  setupTabs(ui.views, (tab) => showView(tab.id.slice('view-'.length)));
  setupTabs(ui.files, (tab) => showFile(tab.dataset.path));
  ui.files.addEventListener('scroll', updateFilesOverflow, { passive: true });
  new ResizeObserver(updateFilesOverflow).observe(ui.files);
  // A mouse wheel scrolls the row of files sideways when it overflows.
  ui.files.addEventListener('wheel', (event) => {
    if (event.deltaX !== 0 || ui.files.scrollWidth <= ui.files.clientWidth) return;
    event.preventDefault();
    ui.files.scrollLeft += event.deltaY;
  });
  ui.generate.addEventListener('click', () => generate());
  ui.cancel.addEventListener('click', cancel);
  ui.retry.addEventListener('click', () => {
    startupRetries += 1;
    start();
  });
  ui.copy.addEventListener('click', copy);
  ui.compileRun.addEventListener('click', compileAndRun);
  ui.showExampleCall.addEventListener('click', () => {
    if (ui.showExampleCall.disabled || !state.result?.files['src/main.rs']) return;
    showFile('src/main.rs');
  });
  ui.cancelRun.addEventListener('click', cancelCompileOrRun);
  ui.restartPython.addEventListener('click', restartPython);
  ui.rustSource.addEventListener('input', editRust);
  document.addEventListener('selectionchange', revealRustCaret);
  ui.rustReset.addEventListener('click', resetRust);
  ui.download.addEventListener('click', download);
  ui.example.addEventListener('change', chooseExample);
  ui.exampleExperiment.addEventListener('click', tryExampleExperiment);
  ui.exampleResult.addEventListener('click', viewExampleResult);
  ui.exampleReset.addEventListener('click', resetExample);
  ui.outline.addEventListener('change', () => {
    const line = Number(ui.outline.value);
    ui.outline.value = '';
    if (line) jumpToLine(line);
  });
  ui.warnings.addEventListener('click', () => {
    showView('report');
    ui.view.querySelector('.warnings-title')?.scrollIntoView({ block: 'start' });
  });
  ui.installForm.addEventListener('submit', (event) => {
    event.preventDefault();
    install();
  });
  for (const chip of document.querySelectorAll('[data-requirement]')) {
    chip.addEventListener('click', async () => {
      ui.requirement.value = chip.dataset.requirement;
      markUserEdit();
      await install();
      if (state.installed) generate();
    });
  }
  for (const chip of document.querySelectorAll('[data-modules]')) {
    chip.addEventListener('click', () => {
      ui.modules.value = chip.dataset.modules;
      markUserEdit();
      generate();
    });
  }
  ui.requirement.addEventListener('input', () => {
    markUserEdit();
    state.installed = null;
    ui.installed.hidden = true;
  });
  ui.modules.addEventListener('input', () => {
    markUserEdit();
    scheduleLive();
  });
  ui.source.addEventListener('input', () => {
    markUserEdit();
    renderEditor();
    revealCaret();
  });
  document.addEventListener('selectionchange', revealCaret);
  for (const field of [ui.source, ui.sourceModule]) {
    field.addEventListener('input', () => {
      ui.example.value = matchingExample();
      markUserEdit();
      updateExampleGuide();
      if (state.mode === 'source') scheduleLive();
    });
  }
  ui.installedModules.addEventListener('change', () => {
    markUserEdit();
    scheduleLive(0);
  });
  for (const input of optionInputs()) {
    input.addEventListener(input.type === 'checkbox' ? 'change' : 'input', () => {
      markUserEdit();
      updateOptionsSummary();
      scheduleLive(input.type === 'checkbox' ? 0 : LIVE_DELAY);
    });
  }
  ui.optionsReset.addEventListener('click', () => {
    markUserEdit();
    setOptions(state.defaults);
    scheduleLive(0);
  });
  ui.live.addEventListener('change', () => {
    markUserEdit(false);
    updateExampleGuide();
    if (ui.live.checked && state.stale) scheduleLive(0);
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      generate();
    } else if (event.key === 'Escape' && state.runBusy) {
      cancelCompileOrRun();
    } else if (event.key === 'Escape' && state.busy) {
      cancel();
    }
  });
}

async function fetchConfig() {
  const response = await fetch('playground.json', { cache: 'no-cache' });
  if (!response.ok) throw new Error(`playground.json: HTTP ${response.status}`);
  const config = await response.json();
  if (!config || typeof config !== 'object' || typeof config.build !== 'string' || typeof config.generator !== 'string' || typeof config.pyodide_url !== 'string' || typeof config.wheel !== 'string') {
    throw new Error('playground.json: missing required build, generator, pyodide_url, or wheel field');
  }
  state.config = config;
  updateCompilerCost();
}

async function restoreInputsOnce() {
  if (inputRestored) return;
  const params = new URLSearchParams(location.hash.slice(1));
  if (!state.editedBeforeReady) {
    initial = await loadFromHash();
    if (state.editedBeforeReady) restorePreReadySnapshot();
  } else {
    initial = { mode: state.mode, params, modules: (params.get('modules') ?? '').split(/\s+/).filter(Boolean) };
  }
  inputRestored = true;
}

async function start() {
  if (startupBusy || state.busy) return;
  startupBusy = true;
  updateExampleGuide();
  ui.retry.disabled = true;
  showMessage('');
  setStatus('loading', 'Preparing Python…');
  try {
    if (!state.config) await fetchConfig();
    await restoreInputsOnce();
    const { build, generator } = state.config;
    ui.buildInfo.textContent = `pyo3_bindgen ${generator} · build ${build.slice(0, 7)}`;
    // Protocol 1 means the page shell is ready; Python may still be loading.
    if (!hostReadySent) {
      notifyHost('wasm-demo-ready', { build });
      hostReadySent = true;
    }
    await boot();
    const hadLostPackages = packagesLost;
    packagesLost = false;
    if (!initialOptionsApplied) {
      // Options stay inert until defaults exist, so apply the link or example
      // once here; later retries never replace options the visitor changed.
      setOptions(state.defaults);
      if (initial.params.has('opt') || initial.params.has('skip')) optionsFromParams(initial.params);
      else {
        const selectedExample = EXAMPLES[initial.params.get('example')] ?? EXAMPLES[ui.example.value];
        if (selectedExample?.options) setOptions({ ...state.defaults, ...selectedExample.options });
      }
      initialOptionsApplied = true;
    }
    showMessage('');
    if (startupRetries === 0 && !state.editedBeforeReady) {
      if (initial.mode === 'package') {
        await install();
        if (state.installed && initial.modules?.length) {
          for (const box of ui.installedModules.querySelectorAll('input')) box.checked = initial.modules.includes(box.value);
        }
        if (state.installed) await generate();
      } else {
        await generate();
      }
    } else if (hadLostPackages) {
      showNote('Python is ready again. Install the package again before generating package bindings.');
    } else if (!state.result) {
      showNote('Python is ready. Review your input, then choose Generate Rust when you are ready to run it.');
    }
  } catch (error) {
    if (error.cancelled) return;
    state.ready = false;
    stopWorker(error);
    if (state.config) failStep();
    else setStep('python', true);
    setStatus('error', state.config ? 'Python failed to start' : 'Failed to load');
    showMessage(state.config
      ? `Python could not start: ${error.message} Retry to try loading Pyodide, the generator, and Python glue again.`
      : `The playground could not load its configuration (${error.message}). Retry to fetch it again.`);
    showRetry(true);
    updateButtons();
    notifyHost('wasm-demo-error', {
      ...(state.config?.build ? { build: state.config.build } : { phase: 'startup' }),
      message: error.message,
    });
  } finally {
    startupBusy = false;
    ui.retry.disabled = false;
    updateButtons();
    updateExampleGuide();
  }
}

wire();
for (const option of ui.example.options) {
  if (Object.hasOwn(EXAMPLES, option.value)) option.textContent = EXAMPLES[option.value].label;
}
if (ui.example.value !== DEFAULT_EXAMPLE) ui.example.value = DEFAULT_EXAMPLE;
ui.sourceModule.value = DEFAULT_EXAMPLE;
updateExampleGuide();
updateButtons();
updateCompilerCost();
start();
