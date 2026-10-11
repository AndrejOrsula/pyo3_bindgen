import { highlightPython, highlightRust } from '../highlight.js';

const EXPECTED_IDS = ['greeting', 'shapes', 'palette'];
const ARCHIVES = Object.freeze({
  greeting: 'archives/greeting.zip',
  shapes: 'archives/shapes.zip',
  palette: 'archives/palette.zip',
});
const MAX_MANIFEST_BYTES = 1024 * 1024;

const tabs = document.getElementById('example-tabs');
const content = document.getElementById('example-content');
const examples = document.getElementById('examples');
const loading = document.getElementById('loading');
const errorBox = document.getElementById('load-error');

function fail(message) {
  loading.hidden = true;
  examples.hidden = true;
  errorBox.textContent = message;
  errorBox.hidden = false;
}

function validateManifest(manifest) {
  if (!manifest || typeof manifest !== 'object' || manifest.schema_version !== 1) {
    throw new Error('The curated examples file has an unsupported format.');
  }
  if (!manifest.generator || typeof manifest.generator.version !== 'string' ||
      typeof manifest.generator.pyo3 !== 'string' || typeof manifest.generator.source_sha256 !== 'string' ||
      !/^[a-f0-9]{64}$/.test(manifest.generator.source_sha256) || typeof manifest.python !== 'string') {
    throw new Error('The curated examples file is missing generator provenance.');
  }
  if (!Array.isArray(manifest.examples) || manifest.examples.length !== EXPECTED_IDS.length ||
      manifest.examples.some((example, index) => !example || example.id !== EXPECTED_IDS[index])) {
    throw new Error('The curated examples file is incomplete.');
  }
  for (const example of manifest.examples) {
    if (typeof example.title !== 'string' || typeof example.summary !== 'string' ||
        typeof example.python !== 'string' || !example.python.trim() ||
        typeof example.rust !== 'string' || !example.rust.trim() ||
        typeof example.call !== 'string' || !example.call.trim() ||
        typeof example.expected_output !== 'string' || !example.expected_output.trim() ||
        !Array.isArray(example.limitations) || example.limitations.some((item) => typeof item !== 'string') ||
        !Array.isArray(example.options) || example.options.some((option) => typeof option !== 'string')) {
      throw new Error(`Example “${example.id}” is missing required generated output or details.`);
    }
    if (example.archive !== ARCHIVES[example.id]) {
      throw new Error(`Example “${example.id}” has an unlisted starter archive.`);
    }
  }
  return manifest;
}

async function loadManifest() {
  const response = await fetch('./examples.json', { cache: 'no-store' });
  if (!response.ok) throw new Error(`Could not load curated examples (${response.status}).`);
  const bytes = await response.arrayBuffer();
  if (bytes.byteLength > MAX_MANIFEST_BYTES) {
    throw new Error('The curated examples file exceeds the 1 MiB size limit.');
  }
  let manifest;
  try {
    manifest = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  } catch {
    throw new Error('The curated examples file is not valid UTF-8 JSON.');
  }
  return validateManifest(manifest);
}

function codeCard(title, value, kind, extraClass = '') {
  const section = document.createElement('section');
  section.className = 'card';
  const header = document.createElement('div');
  header.className = 'card-head';
  const heading = document.createElement('h3');
  heading.textContent = title;
  const language = document.createElement('span');
  language.textContent = kind === 'python' ? 'Python' : 'Rust';
  header.append(heading, language);
  const pre = document.createElement('pre');
  pre.className = `code ${extraClass}`.trim();
  pre.tabIndex = 0;
  const code = document.createElement('code');
  code.append(kind === 'python' ? highlightPython(value) : highlightRust(value));
  pre.append(code);
  section.append(header, pre);
  return section;
}

function renderExample(example, manifest) {
  content.replaceChildren();
  content.setAttribute('aria-busy', 'false');
  content.setAttribute('aria-labelledby', `tab-${example.id}`);
  const summary = document.createElement('section');
  summary.className = 'summary';
  const heading = document.createElement('h2');
  heading.textContent = example.title;
  const description = document.createElement('p');
  description.textContent = example.summary;
  const liveLink = document.createElement('a');
  liveLink.className = 'curated-live-link';
  liveLink.href = `../#example=${encodeURIComponent(example.id)}`;
  liveLink.textContent = `Try ${example.title} live`;
  const metadata = document.createElement('div');
  metadata.className = 'metadata';
  for (const [label, value] of [
    ['Python', manifest.python],
    ['Generator', `pyo3_bindgen ${manifest.generator.version}`],
    ['PyO3', manifest.generator.pyo3],
    ['Generator source tree SHA-256', manifest.generator.source_sha256],
  ]) {
    const item = document.createElement('span');
    const strong = document.createElement('strong');
    strong.textContent = `${label}: `;
    item.append(strong, document.createTextNode(value));
    metadata.append(item);
  }
  const download = document.createElement('a');
  download.className = 'download';
  download.href = ARCHIVES[example.id];
  download.download = `${example.id}.zip`;
  download.textContent = 'Download pinned starter';
  const optionTitle = document.createElement('p');
  optionTitle.className = 'metadata';
  optionTitle.textContent = 'Generation options';
  const optionList = document.createElement('ul');
  optionList.className = 'option-list';
  for (const option of example.options) {
    const item = document.createElement('li');
    item.textContent = option;
    optionList.append(item);
  }
  const details = document.createElement('details');
  details.className = 'generation-details';
  const detailsLabel = document.createElement('summary');
  detailsLabel.textContent = 'Provenance and generation options';
  details.append(detailsLabel, metadata, optionTitle, optionList);
  const result = document.createElement('div');
  result.className = 'result';
  const resultItem = document.createElement('div');
  const resultTitle = document.createElement('h4');
  resultTitle.textContent = 'Expected result';
  const resultValue = document.createElement('pre');
  resultValue.textContent = example.expected_output;
  resultItem.append(resultTitle, resultValue);
  result.append(resultItem);
  summary.append(heading, description, liveLink, download, result, details);

  const columns = document.createElement('div');
  columns.className = 'columns';
  const left = document.createElement('div');
  left.className = 'column';
  left.append(codeCard('Python input', example.python, 'python'));
  const limits = document.createElement('section');
  limits.className = 'card';
  const limitsHeader = document.createElement('div');
  limitsHeader.className = 'card-head';
  const limitsTitle = document.createElement('h3');
  limitsTitle.textContent = 'Limitations';
  limitsHeader.append(limitsTitle);
  const list = document.createElement('ul');
  list.className = 'limitations';
  for (const limitation of example.limitations) {
    const item = document.createElement('li');
    item.textContent = limitation;
    list.append(item);
  }
  limits.append(limitsHeader, list);
  left.append(limits);

  const right = document.createElement('div');
  right.className = 'column';
  right.append(codeCard('Complete generated Rust', example.rust, 'rust', 'generated-rust'));
  const callCard = document.createElement('section');
  callCard.className = 'card';
  const callHead = document.createElement('div');
  callHead.className = 'card-head';
  const callTitle = document.createElement('h3');
  callTitle.textContent = 'Runnable Rust call';
  const callLanguage = document.createElement('span');
  callLanguage.textContent = 'Consumer example';
  callHead.append(callTitle, callLanguage);
  const call = document.createElement('pre');
  call.className = 'code call-code';
  call.tabIndex = 0;
  const callText = document.createElement('code');
  callText.append(highlightRust(example.call));
  call.append(callText);
  callCard.append(callHead, call);
  right.append(callCard);
  columns.append(left, right);
  content.append(summary, columns);
}

function exampleFromHash(manifest) {
  const id = new URLSearchParams(location.hash.slice(1)).get('example');
  const index = manifest.examples.findIndex((example) => example.id === id);
  return index < 0 ? 0 : index;
}

function writeExampleHash(example, replace = false) {
  const hash = `#example=${encodeURIComponent(example.id)}`;
  if (location.hash === hash) return;
  history[replace ? 'replaceState' : 'pushState'](null, '', hash);
}

function selectExample(manifest, index, focus = false, { updateHash = true, replaceHash = false } = {}) {
  const buttons = [...tabs.querySelectorAll('[role="tab"]')];
  buttons.forEach((button, buttonIndex) => {
    const selected = buttonIndex === index;
    button.setAttribute('aria-selected', String(selected));
    button.tabIndex = selected ? 0 : -1;
  });
  if (focus) buttons[index].focus();
  const example = manifest.examples[index];
  renderExample(example, manifest);
  if (updateHash) writeExampleHash(example, replaceHash);
}

async function start() {
  try {
    const manifest = await loadManifest();
    for (const [index, example] of manifest.examples.entries()) {
      const button = document.createElement('button');
      button.type = 'button';
      button.id = `tab-${example.id}`;
      button.setAttribute('role', 'tab');
      button.setAttribute('aria-controls', 'example-content');
      button.setAttribute('aria-selected', String(index === 0));
      button.tabIndex = index === 0 ? 0 : -1;
      button.textContent = example.title;
      button.addEventListener('click', () => selectExample(manifest, index));
      button.addEventListener('keydown', (event) => {
        let next = index;
        if (event.key === 'ArrowRight') next = (index + 1) % manifest.examples.length;
        else if (event.key === 'ArrowLeft') next = (index + manifest.examples.length - 1) % manifest.examples.length;
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = manifest.examples.length - 1;
        else return;
        event.preventDefault();
        selectExample(manifest, next, true);
      });
      tabs.append(button);
    }
    loading.hidden = true;
    examples.hidden = false;
    selectExample(manifest, exampleFromHash(manifest), false, { replaceHash: true });
    const restoreFromLocation = () => {
      selectExample(manifest, exampleFromHash(manifest), false, { replaceHash: true });
    };
    window.addEventListener('popstate', restoreFromLocation);
    window.addEventListener('hashchange', restoreFromLocation);
  } catch (error) {
    fail(error instanceof Error ? error.message : 'Could not display curated examples.');
  }
}

start();
