// The overview: each Python item beside the Rust it became. It reads the
// generated bindings.rs, which prettyplease formats with four-space indents and
// closing braces at the indent of their item, and pairs its items with the
// Python signatures that the worker reads with `inspect`.

import { highlightPython, highlightRust } from './highlight.js';

// The overview lists at most this many rows; the code view has the rest.
const ROW_LIMIT = 1500;

// Helpers that every generated class or enum has.
const BOILERPLATE = new Set(['from_py_any', 'bind', 'as_any', 'clone_ref', 'into_py_any', 'as_py_name', 'from_py_name']);

// A header ends with its body's brace, an empty body, or a semicolon.
const HEADER_END = /[{;]$|\{\}$/;
const ALIAS = /^#\[doc\(alias = "([^"]*)"\)\]$/;

const indentOf = (line) => line.length - line.trimStart().length;
const bare = (name) => name.replace(/^r#/, '');

/* ---------- Parsing ---------- */

// Calls `visit` for each item between lines `from` and `to` (exclusive) at
// `indent`, with its header joined onto one line, its doc comment and aliases,
// and the lines of its body.
function scan(lines, from, to, indent, visit) {
  let docs = [];
  let aliases = [];
  const closing = ' '.repeat(indent) + '}';
  for (let index = from; index < to; index++) {
    const line = lines[index];
    if (indentOf(line) !== indent || line.trim() === '') continue;
    const text = line.trim();
    if (text.startsWith('///')) {
      docs.push(text.replace(/^\/\/\/ ?/, ''));
      continue;
    }
    if (text.startsWith('#')) {
      const alias = ALIAS.exec(text);
      if (alias) aliases.push(alias[1]);
      // Attributes that span lines end with `)]` at their own indent.
      if (!text.endsWith(']')) while (index + 1 < to && lines[index + 1] !== ' '.repeat(indent) + ')]') index++;
      if (!text.endsWith(']')) index++;
      continue;
    }
    let end = index;
    while (end + 1 < to && !HEADER_END.test(lines[end].trimEnd())) end++;
    const header = lines.slice(index, end + 1).map((part) => part.trim()).join(' ');
    let close = end;
    if (header.endsWith('{')) {
      close = end + 1;
      while (close < to && lines[close] !== closing) close++;
    }
    visit({ header, line: index + 1, docs, aliases, body: [end + 1, close] });
    docs = [];
    aliases = [];
    index = close;
  }
}

function newModule(path, line, docs) {
  return { path, line, doc: docs[0] ?? '', structs: [], enums: [], flags: [], functions: [], types: [], builders: new Map(), traits: new Map(), impls: new Map(), inherent: new Map(), constants: new Map(), implDocs: new Map() };
}

function members(lines, body, indent) {
  const found = [];
  scan(lines, body[0], body[1], indent, (item) => {
    const match = /^(?:pub )?fn (?:r#)?(\w+)/.exec(item.header);
    if (match) found.push({ name: match[1], signature: rustSignature(item.header), line: item.line, doc: item.docs[0] ?? '' });
  });
  return found;
}

// The members of a Python flag, which become constants with the Python name as
// their alias; aliases of other members have none.
function flagMembers(lines, body, indent) {
  const found = [];
  scan(lines, body[0], body[1], indent, (item) => {
    const match = /^pub const (?:r#)?(\w+):/.exec(item.header);
    if (match && item.aliases.length) found.push({ rust: match[1], python: item.aliases[0], line: item.line });
  });
  return found;
}

function enumVariants(lines, body, indent) {
  const variants = [];
  let alias = null;
  for (let index = body[0]; index < body[1]; index++) {
    const text = lines[index].trim();
    if (indentOf(lines[index]) !== indent) continue;
    const match = ALIAS.exec(text);
    if (match) alias = match[1];
    const variant = /^(?:r#)?(\w+)\s*(?:=\s*[^,]+)?,?$/.exec(text);
    if (variant && !text.startsWith('#') && !text.startsWith('/')) {
      variants.push({ rust: variant[1], python: alias ?? variant[1], line: index + 1 });
      alias = null;
    }
  }
  return variants;
}

function parseModule(lines, from, to, indent, module, modules) {
  scan(lines, from, to, indent, (item) => {
    const { header } = item;
    let match;
    if ((match = /^pub mod (?:r#)?(\w+) \{$/.exec(header))) {
      const child = newModule(`${module.path}.${match[1]}`, item.line, item.docs);
      modules.push(child);
      parseModule(lines, item.body[0], item.body[1], indent + 4, child, modules);
    } else if ((match = /^pub struct (?:r#)?(\w+)\(u64\);$/.exec(header))) {
      module.flags.push({ name: match[1], line: item.line, doc: item.docs[0] ?? '', flags: true });
    } else if ((match = /^pub struct (?:r#)?(\w+)(<[^{;]*>)?/.exec(header))) {
      if (match[2]) module.builders.set(match[1], item.line);
      else module.structs.push({ name: match[1], line: item.line, doc: item.docs[0] ?? '' });
    } else if ((match = /^pub enum (?:r#)?(\w+)/.exec(header))) {
      module.enums.push({ name: match[1], line: item.line, doc: item.docs[0] ?? '', variants: enumVariants(lines, item.body, indent + 4) });
    } else if ((match = /^pub trait (?:r#)?(\w+)/.exec(header))) {
      module.traits.set(match[1], { line: item.line, owner: item.aliases[0] ?? match[1].replace(/Methods$/, ''), members: members(lines, item.body, indent + 4) });
    } else if ((match = /^impl(?:<[^>]*>)? (?:r#)?(\w+) for (?:&'a )?(?:r#)?(\w+) \{$/.exec(header))) {
      const list = module.impls.get(match[2]) ?? [];
      list.push({ trait: match[1], line: item.line });
      module.impls.set(match[2], list);
      for (const member of members(lines, item.body, indent + 4)) module.implDocs.set(`${match[1]}.${member.name}`, member);
    } else if ((match = /^impl(?:<[^>]*>)? (?:r#)?(\w+)(?:<[^>]*>)? \{$/.exec(header))) {
      const list = module.inherent.get(match[1]) ?? [];
      list.push(...members(lines, item.body, indent + 4));
      module.inherent.set(match[1], list);
      module.constants.set(match[1], [...(module.constants.get(match[1]) ?? []), ...flagMembers(lines, item.body, indent + 4)]);
    } else if ((match = /^pub fn (?:r#)?(\w+)/.exec(header))) {
      if (match[1] !== 'pyo3_embed_python_source_code') {
        module.functions.push({ name: match[1], signature: rustSignature(header), line: item.line, doc: item.docs[0] ?? '' });
      }
    } else if ((match = /^pub type (?:r#)?(\w+)/.exec(header))) {
      module.types.push({ name: match[1], signature: rustSignature(header), line: item.line, doc: item.docs[0] ?? '' });
    }
  });
}

// Shortens a header for reading: no `pub`, paths, body, or named lifetimes.
export function rustSignature(header) {
  return header
    .replace(/\s*\{$/, '')
    .replace(/;$/, '')
    .replace(/^pub /, '')
    .replace(/^(fn (?:r#)?\w+)<'py>/, '$1')
    .replace(/(?:::)?(?:[a-z_][a-z0-9_]*::)+(?=[A-Za-z_])/g, '')
    .replace(/'py\b/g, "'_")
    .replace(/([(<])\s+/g, '$1')
    .replace(/,?\s+([)>])/g, '$1')
    .replace(/\s+/g, ' ');
}

// Groups `x` and `set_x`, and `f`, `f_builder` and the builder's struct.
function pairAccessors(list) {
  const byName = new Map(list.map((item) => [item.name, item]));
  const rows = [];
  for (const item of list) {
    const getter = item.name.startsWith('set_') ? byName.get(item.name.slice(4)) : null;
    if (getter) continue;
    rows.push({ ...item, setter: byName.get(`set_${item.name}`) ?? null });
  }
  return rows;
}

// The lines of the bindings with each block doc comment, which prettyplease
// writes for multi-line docstrings with their lines unindented, as one line
// comment of its first text followed by blank lines, so that line numbers stay.
function codeLines(code) {
  const lines = code.split('\n');
  for (let index = 0; index < lines.length; index++) {
    const text = lines[index].trim();
    if (!text.startsWith('/*')) continue;
    const parts = [];
    let end = index;
    let rest = text.replace(/^\/\*[*!]?/, '');
    for (;;) {
      const close = rest.indexOf('*/');
      parts.push(close >= 0 ? rest.slice(0, close) : rest);
      if (close >= 0 || end + 1 >= lines.length) break;
      rest = lines[++end];
    }
    lines[index] = `${' '.repeat(indentOf(lines[index]))}/// ${parts.map((part) => part.trim()).find(Boolean) ?? ''}`;
    for (let blank = index + 1; blank <= end; blank++) lines[blank] = '';
    index = end;
  }
  return lines;
}

export function parseBindings(code) {
  const lines = codeLines(code);
  const modules = [];
  scan(lines, 0, lines.length, 0, (item) => {
    const match = /^pub mod (?:r#)?(\w+) \{$/.exec(item.header);
    if (!match) return;
    const module = newModule(match[1], item.line, item.docs);
    modules.push(module);
    parseModule(lines, item.body[0], item.body[1], 4, module, modules);
  });
  return modules.map(assemble);
}

function assemble(module) {
  const classes = module.structs.map((struct) => {
    const traitName = [...module.traits].find(([, trait]) => trait.owner === struct.name)?.[0] ?? `${struct.name}Methods`;
    const trait = module.traits.get(traitName);
    const impls = module.impls.get(struct.name) ?? [];
    const own = (trait?.members ?? []).map((member) => {
      const body = module.implDocs.get(`${traitName}.${member.name}`);
      return { ...member, line: body?.line ?? member.line, doc: body?.doc || member.doc };
    });
    const ownNames = new Set(own.map((member) => member.name));
    const inherited = impls.filter((impl) => impl.trait !== traitName && module.traits.has(impl.trait));
    const inheritedNames = new Set(inherited.flatMap((impl) => module.traits.get(impl.trait).members.map((member) => member.name)));
    const inherent = module.inherent.get(struct.name) ?? [];
    const constructor = inherent.find((member) => member.name === 'new') ?? null;
    // Inherent methods with a trait method's name forward to it.
    const forwarders = inherent.filter((member) => ownNames.has(member.name) || inheritedNames.has(member.name));
    const associated = inherent.filter((member) => member !== constructor && !BOILERPLATE.has(member.name) && !forwarders.includes(member));
    return {
      ...struct,
      constructor,
      methods: pairAccessors([...associated.map((member) => ({ ...member, static: true })), ...own]),
      inherits: inherited.map((impl) => ({ trait: impl.trait, owner: module.traits.get(impl.trait).owner, line: impl.line })),
      direct: forwarders.length > 0,
    };
  });
  const functionNames = new Set(module.functions.map((item) => item.name));
  const builders = new Map();
  for (const item of module.functions) {
    const target = item.name.replace(/_builder$/, '');
    if (target !== item.name && functionNames.has(target)) {
      const struct = [...module.builders.keys()].find((name) => new RegExp(`\\b${name}\\b`).test(item.signature));
      const steps = (module.inherent.get(struct) ?? []).filter((member) => member.name.startsWith('with_') || member.name === 'call');
      builders.set(target, { ...item, steps: steps.map((step) => step.name) });
    }
  }
  const functions = pairAccessors(module.functions.filter((item) => !(item.name.endsWith('_builder') && builders.has(item.name.slice(0, -'_builder'.length)))));
  for (const item of functions) item.builder = builders.get(item.name) ?? null;
  const flags = module.flags.map((item) => ({ ...item, variants: module.constants.get(item.name) ?? [] }));
  const enums = [...module.enums, ...flags].sort((a, b) => a.line - b.line);
  return { path: module.path, line: module.line, doc: module.doc, classes, enums, functions, types: module.types };
}

/* ---------- Python source lines ---------- */

const PYTHON_DEFINITION = /^(?:(?:async\s+)?def|class)\s+([A-Za-z_]\w*)/;
const PYTHON_ASSIGNMENT = /^([A-Za-z_]\w*)\s*(?::(?!=)|=(?!=))/;
const PYTHON_BLOCK = /^(?:else|try|finally|except|case|match|with|if|elif|while|for|lambda)\b/;

// The triple quote that is still open at the end of a line of Python, given
// the one open at its start. Escaped quotes, one-line strings and comments
// neither open nor close one.
function openQuote(text, quote) {
  let single = null;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if ((quote || single) && character === '\\') index += 1;
    else if (quote) {
      if (text.startsWith(quote, index)) {
        quote = null;
        index += 2;
      }
    } else if (single) {
      if (character === single) single = null;
    } else if (character === '#') break;
    else if (text.startsWith('"""', index) || text.startsWith("'''", index)) {
      quote = text.slice(index, index + 3);
      index += 2;
    } else if (character === '"' || character === "'") single = character;
  }
  return quote;
}

// The lines that define the top-level names of pasted Python code and the
// members of its top-level classes, as `name` and `Class.member`. Lines inside
// triple-quoted strings are skipped; the last definition of a name wins, as
// it does in Python.
export function pythonLines(source) {
  const found = new Map();
  let quote = null;
  let owner = null;
  let bodyIndent = null;
  for (const [index, text] of source.split('\n').entries()) {
    const inside = quote;
    quote = openQuote(text, quote);
    if (inside) continue;
    const trimmed = text.trim();
    if (trimmed === '' || trimmed.startsWith('#')) continue;
    const indent = indentOf(text);
    const definition = PYTHON_DEFINITION.exec(trimmed);
    const name = definition?.[1] ?? (PYTHON_BLOCK.test(trimmed) ? null : PYTHON_ASSIGNMENT.exec(trimmed)?.[1]);
    if (indent === 0) {
      owner = null;
      if (name) found.set(name, index + 1);
      if (name && trimmed.startsWith('class')) {
        owner = name;
        bodyIndent = null;
      }
    } else if (owner) {
      bodyIndent ??= indent;
      if (name && indent === bodyIndent && !trimmed.startsWith('class')) found.set(`${owner}.${name}`, index + 1);
    }
  }
  return found;
}

/* ---------- Rendering ---------- */

function element(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.append(...children.filter((child) => child !== null && child !== undefined && child !== ''));
  return node;
}

// Python code with an optional keyword, and a name coloured as `nameKind`.
function pythonCode(text, name, keyword = '', nameKind = 'fn') {
  const code = element('code', 'py');
  if (keyword) code.append(element('span', 'tok-keyword', keyword), ' ');
  if (name) code.append(element('span', `tok-${nameKind}`, name));
  if (text) code.append(highlightPython(text));
  return code;
}

// Rust code, with the `py` token that every call takes dimmed.
function rustCode(text) {
  const code = element('code', 'rs');
  for (const part of text.split(/(py: Python<'_>)/)) {
    if (part === "py: Python<'_>") code.append(element('span', 'api-py-token', highlightRust(part)));
    else if (part) code.append(highlightRust(part));
  }
  return code;
}

// Finds the Python entry of a member, also on the bases of its class.
function pythonMember(api, module, owner, name, seen = new Set()) {
  const entry = api[`${module}.${owner}.${name}`];
  if (entry || seen.has(owner)) return entry;
  seen.add(owner);
  for (const base of api[`${module}.${owner}`]?.bases ?? []) {
    const found = pythonMember(api, module, base, name, seen);
    if (found) return found;
  }
  return undefined;
}

function pythonSide(entry, name) {
  if (!entry) return pythonCode('', name, '', 'name');
  switch (entry.kind) {
    case 'function':
    case 'method':
    case 'staticmethod':
    case 'classmethod':
      return pythonCode(entry.signature ?? '(…)', name, 'def');
    case 'property':
    case 'attribute':
    case 'value':
      return pythonCode(entry.type ? `: ${entry.type}` : '', name, '', 'name');
    default:
      return pythonCode('', name, '', 'name');
  }
}

const TAGS = { property: 'property', attribute: 'field', value: 'attribute', staticmethod: 'static', classmethod: 'class method' };

// `sources` maps the names of the pasted module `sourceModule` to their lines,
// which `onSource` marks while a row has the pointer or focus.
export function renderOverview(modules, api, { onJump, file, sources = null, sourceModule = null, onSource = () => {} }) {
  const wrap = element('div', 'overview');
  const sourceLine = (module, key) => (sources && module === sourceModule ? (sources.get(key) ?? null) : null);
  const memberLine = (module, owner, name, seen = new Set()) => {
    const line = sourceLine(module, `${owner}.${name}`);
    if (line || seen.has(owner)) return line;
    seen.add(owner);
    for (const base of api[`${module}.${owner}`]?.bases ?? []) {
      const found = memberLine(module, base, name, seen);
      if (found) return found;
    }
    return null;
  };
  wrap.append(element('p', 'api-columns', element('span', null, 'Python'), element('span', null, 'Rust')));
  let rows = 0;
  let hidden = 0;
  const row = (python, rust, line, { doc = '', tag = '', extra = [], className = '', source = null } = {}) => {
    if (rows >= ROW_LIMIT) {
      hidden++;
      return null;
    }
    rows++;
    const signature = element('span', 'api-sig', tag ? element('span', 'api-tag', tag) : null, python);
    const button = element(
      'button',
      `api-row ${className}`.trim(),
      element('span', 'api-py', signature, doc ? element('span', 'api-doc', doc) : null),
      element('span', 'api-rs', rust, ...extra),
    );
    button.type = 'button';
    button.title = `Show in ${file}, line ${line}`;
    button.dataset.line = String(line);
    button.addEventListener('click', () => onJump(line));
    if (source) {
      button.dataset.source = String(source);
      for (const type of ['pointerenter', 'focus']) button.addEventListener(type, () => onSource(source));
      for (const type of ['pointerleave', 'blur']) button.addEventListener(type, () => onSource(null));
    }
    return element('li', null, button);
  };
  const add = (list, item) => item && list.append(item);

  for (const module of modules) {
    const section = element('section', 'api-module');
    const moduleDoc = api[module.path]?.doc || module.doc;
    const title = element('h3', 'api-module-title', element('span', 'tok-keyword', 'mod'), ' ', element('span', 'api-module-name', module.path.replace(/\./g, '::')));
    section.append(title);
    if (moduleDoc) section.append(element('p', 'api-module-doc', moduleDoc));

    for (const cls of module.classes) {
      const python = api[`${module.path}.${cls.name}`];
      const card = element('ul', 'api-card');
      const bases = python?.bases?.length ? `(${python.bases.join(', ')})` : '';
      add(card, row(pythonCode(bases, cls.name, 'class', 'type'), rustCode(`struct ${cls.name}`), cls.line, { doc: python?.doc || cls.doc, className: 'api-head', source: sourceLine(module.path, cls.name) }));
      if (cls.constructor) {
        const rust = rustCode(cls.constructor.signature.replace(/^fn new/, `${cls.name}::new`));
        add(card, row(pythonCode(python?.signature ?? '(…)', cls.name, '', 'type'), rust, cls.constructor.line, { tag: 'new', source: memberLine(module.path, cls.name, '__init__') ?? sourceLine(module.path, cls.name) }));
      }
      for (const member of cls.methods) {
        const entry = pythonMember(api, module.path, cls.name, bare(member.name));
        const extra = member.setter ? [rustCode(member.setter.signature)] : [];
        const tag = TAGS[entry?.kind] ?? (member.static ? 'static' : '');
        add(card, row(pythonSide(entry, bare(member.name)), rustCode(member.signature), member.line, { doc: entry?.doc || member.doc, tag, extra, source: memberLine(module.path, cls.name, bare(member.name)) }));
      }
      for (const impl of cls.inherits) {
        const python = element('span', 'api-muted', 'methods of ', element('code', null, impl.owner));
        add(card, row(python, rustCode(`impl ${impl.trait} for ${cls.name}`), impl.line, { className: 'api-inherit', tag: 'inherits' }));
      }
      section.append(card);
    }

    for (const item of module.enums) {
      const python = api[`${module.path}.${item.name}`];
      const card = element('ul', 'api-card');
      const bases = python?.bases?.length ? `(${python.bases.join(', ')})` : '';
      add(card, row(pythonCode(bases, item.name, 'class', 'type'), rustCode(item.flags ? `struct ${item.name}(u64)` : `enum ${item.name}`), item.line, { doc: python?.doc || item.doc, className: 'api-head', source: sourceLine(module.path, item.name) }));
      const values = new Map((python?.members ?? []).map((text) => [text.split(' = ')[0], text]));
      for (const variant of item.variants) {
        add(card, row(pythonCode(values.get(variant.python) ?? variant.python, ''), rustCode(`${item.name}::${variant.rust}`), variant.line, { tag: 'member', source: sourceLine(module.path, `${item.name}.${variant.python}`) }));
      }
      section.append(card);
    }

    const loose = element('ul', 'api-card api-loose');
    for (const item of module.functions) {
      const entry = api[`${module.path}.${bare(item.name)}`];
      const extra = [];
      if (item.setter) extra.push(rustCode(item.setter.signature));
      if (item.builder) {
        extra.push(rustCode(item.builder.signature), element('span', 'api-steps', ...item.builder.steps.map((step) => element('code', null, `.${step}(${step === 'call' ? '' : '…'})`))));
      }
      const tag = entry?.kind === 'value' || (!entry && item.setter) ? 'attribute' : '';
      add(loose, row(pythonSide(entry, bare(item.name)), rustCode(item.signature), item.line, { doc: entry?.doc || item.doc, tag, extra, source: sourceLine(module.path, bare(item.name)) }));
    }
    for (const item of module.types) {
      add(loose, row(pythonCode('', bare(item.name), '', 'type'), rustCode(item.signature), item.line, { doc: item.doc, tag: 'type', source: sourceLine(module.path, bare(item.name)) }));
    }
    if (loose.childElementCount) section.append(loose);
    if (!section.querySelector('.api-card')) section.append(element('p', 'api-muted', 'Nothing to bind in this module.'));
    wrap.append(section);
  }
  wrap.addEventListener('pointerleave', () => onSource(null));
  if (hidden) wrap.append(element('p', 'view-intro', `… and ${hidden.toLocaleString('en')} more items, which the code view lists.`));
  return wrap;
}

export function overviewText(modules, api) {
  const lines = [];
  for (const module of modules) {
    lines.push(`mod ${module.path.replace(/\./g, '::')}`);
    for (const cls of module.classes) {
      lines.push(`class ${cls.name}\tstruct ${cls.name}`);
      if (cls.constructor) lines.push(`${cls.name}${api[`${module.path}.${cls.name}`]?.signature ?? ''}\t${cls.constructor.signature}`);
      for (const member of cls.methods) {
        const entry = pythonMember(api, module.path, cls.name, bare(member.name));
        lines.push(`${pythonText(entry, bare(member.name))}\t${member.signature}`);
      }
    }
    for (const item of module.enums) {
      lines.push(`class ${item.name}\tenum ${item.name}`);
      for (const variant of item.variants) lines.push(`${variant.python}\t${item.name}::${variant.rust}`);
    }
    for (const item of module.functions) lines.push(`${pythonText(api[`${module.path}.${bare(item.name)}`], bare(item.name))}\t${item.signature}`);
    for (const item of module.types) lines.push(`${bare(item.name)}\t${item.signature}`);
  }
  return `${lines.join('\n')}\n`;
}

function pythonText(entry, name) {
  if (!entry) return name;
  if (entry.signature !== undefined) return `def ${name}${entry.signature ?? '(…)'}`;
  return entry.type ? `${name}: ${entry.type}` : name;
}
