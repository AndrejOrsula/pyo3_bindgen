// Small Rust, Python, TOML, and Markdown highlighters for the generated code,
// starter files, and pasted source. They build text nodes and spans, never
// HTML, so source text cannot inject markup.

const KEYWORDS = new Set(
  ('as async await break const continue crate dyn else enum extern false fn for if impl in let loop ' +
    'match mod move mut pub ref return self Self static struct super trait true type unsafe use where while')
    .split(' '),
);

const TOKEN = new RegExp(
  [
    String.raw`(?<comment>\/\/[^\n]*|\/\*[\s\S]*?\*\/)`,
    String.raw`(?<string>b?r(?<hashes>#*)"[\s\S]*?"\k<hashes>|b?"(?:\\[\s\S]|[^"\\])*"|'(?:\\.|[^'\\])')`,
    String.raw`(?<lifetime>'[A-Za-z_]\w*)`,
    String.raw`(?<attr>#!?\[[^\]\n]*\]?)`,
    String.raw`(?<macro>\b[a-z_]\w*!)`,
    String.raw`(?<number>\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?(?:[iuf](?:8|16|32|64|128|size))?\b)`,
    String.raw`(?<word>\b(?:r#)?[A-Za-z_]\w*)`,
  ].join('|'),
  'g',
);

const PYTHON_KEYWORDS = new Set(
  ('False None True and as assert async await break class continue def del elif else except finally for ' +
    'from global if import in is lambda nonlocal not or pass raise return try while with yield')
    .split(' '),
);
const PYTHON_BUILTINS = new Set(
  'bool bytes complex dict float frozenset int list object set str tuple type self cls super len range print isinstance'.split(' '),
);

// Strings may be unterminated while the visitor types; they then end at the
// end of the line, or for triple quotes at the end of the code.
const PYTHON_TOKEN = new RegExp(
  [
    String.raw`(?<comment>#[^\n]*)`,
    String.raw`(?<string>[rRbBuUfF]{0,2}(?:"""(?:\\[\s\S]|[^\\])*?(?:"""|$(?![\s\S]))|'''(?:\\[\s\S]|[^\\])*?(?:'''|$(?![\s\S]))|"(?:\\.|[^"\\\n])*"?|'(?:\\.|[^'\\\n])*'?))`,
    String.raw`(?<decorator>@[A-Za-z_][\w.]*)`,
    String.raw`(?<number>\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?j?\b)`,
    String.raw`(?<word>\b[A-Za-z_]\w*)`,
  ].join('|'),
  'g',
);

function span(kind, text) {
  const element = document.createElement('span');
  element.className = `tok-${kind}`;
  element.textContent = text;
  return element;
}

function highlight(code, pattern, classify) {
  const fragment = document.createDocumentFragment();
  let last = 0;
  let previous = null;
  for (const match of code.matchAll(pattern)) {
    const kind = classify(match.groups, previous);
    previous = match.groups.word ?? null;
    if (!kind) continue;
    if (match.index > last) fragment.append(code.slice(last, match.index));
    fragment.append(span(kind, match[0]));
    last = match.index + match[0].length;
  }
  if (last < code.length) fragment.append(code.slice(last));
  return fragment;
}

export function highlightRust(code) {
  return highlight(code, TOKEN, (groups, previous) => {
    if (groups.comment) return 'comment';
    if (groups.string) return 'string';
    if (groups.lifetime) return 'type';
    if (groups.attr) return 'attr';
    if (groups.macro) return 'macro';
    if (groups.number) return 'number';
    if (groups.word) {
      if (KEYWORDS.has(groups.word)) return 'keyword';
      if (previous === 'fn') return 'fn';
      if (/^(?:r#)?[A-Z]/.test(groups.word)) return 'type';
    }
    return null;
  });
}

export function highlightPython(code) {
  return highlight(code, PYTHON_TOKEN, (groups, previous) => {
    if (groups.comment) return 'comment';
    if (groups.string) return 'string';
    if (groups.decorator) return 'macro';
    if (groups.number) return 'number';
    if (groups.word) {
      if (previous === 'def') return 'fn';
      if (previous === 'class') return 'type';
      if (PYTHON_KEYWORDS.has(groups.word)) return 'keyword';
      if (PYTHON_BUILTINS.has(groups.word)) return 'builtin';
      if (/^[A-Z]/.test(groups.word)) return 'type';
    }
    return null;
  });
}

const TOML_TOKEN = new RegExp(
  [
    String.raw`(?<comment>#[^\n]*)`,
    String.raw`(?<string>"""(?:\\[\s\S]|(?!""")[^\\])*?"""|'''[\s\S]*?'''|"(?:\\.|[^"\\\n])*"|'[^'\n]*')`,
    String.raw`(?<table>^[ \t]*\[{1,2}[^\]\n]*\]{1,2})`,
    String.raw`(?<lineKey>^[ \t]*(?:[A-Za-z0-9_-]+\.)*[A-Za-z0-9_-]+(?=[ \t]*=))`,
    String.raw`(?<inlineKey>(?<=[{,])[ \t]*(?:[A-Za-z0-9_-]+\.)*[A-Za-z0-9_-]+(?=[ \t]*=))`,
    String.raw`(?<boolean>\b(?:true|false)\b)`,
    String.raw`(?<number>\b[+-]?(?:0[xX][0-9A-Fa-f](?:_?[0-9A-Fa-f])*|0[oO][0-7](?:_?[0-7])*|0[bB][01](?:_?[01])*|(?:\d(?:_?\d)*(?:\.(?!\.)\d(?:_?\d)*)?(?:[eE][+-]?\d(?:_?\d)*)?))\b)`,
  ].join('|'),
  'gm',
);

export function highlightToml(code) {
  return highlight(code, TOML_TOKEN, (groups) => {
    if (groups.comment) return 'comment';
    if (groups.string) return 'string';
    if (groups.table) return 'table';
    if (groups.lineKey || groups.inlineKey) return 'key';
    if (groups.boolean) return 'boolean';
    if (groups.number) return 'number';
    return null;
  });
}

const MARKDOWN_INLINE = new RegExp(
  [
    String.raw`(?<link>\[[^\[\]\n]+\]\([^()\[\]\n]*\))`,
    String.raw`(?<inline>\x60+[^\x60\n]*\x60+)`,
    String.raw`(?<strong>\*\*[^*\n]+\*\*|__[^_\n]+__)`,
    String.raw`(?<emphasis>\*[^*\n]+\*|_[^_\n]+_)`,
  ].join('|'),
  'g',
);

function appendMarkdownLine(fragment, line, content, kind) {
  if (kind) {
    fragment.append(span(kind, line));
    return;
  }
  const newline = line.endsWith('\n') ? '\n' : '';
  fragment.append(highlight(content, MARKDOWN_INLINE, (groups) => {
    if (groups.link) return 'link';
    if (groups.inline) return 'inline';
    if (groups.strong) return 'strong';
    if (groups.emphasis) return 'emphasis';
    return null;
  }));
  if (newline) fragment.append(newline);
}

export function highlightMarkdown(code) {
  const fragment = document.createDocumentFragment();
  let offset = 0;
  let fence = null;
  while (offset < code.length) {
    const newline = code.indexOf('\n', offset);
    const end = newline < 0 ? code.length : newline + 1;
    const line = code.slice(offset, end);
    const content = line.endsWith('\n') ? line.slice(0, -1) : line;
    if (fence) {
      fragment.append(span('fenced', line));
      const closing = new RegExp(`^ {0,3}${fence.character}{${fence.length},}\\s*$`);
      if (closing.test(content)) fence = null;
    } else {
      const opening = /^( {0,3})(`{3,}|~{3,})[^\n]*$/.exec(content);
      if (opening) {
        fence = { character: opening[2][0], length: opening[2].length };
        fragment.append(span('fenced', line));
      } else if (/^ {0,3}#{1,6}(?:\s|$)/.test(content)) {
        appendMarkdownLine(fragment, line, content, 'heading');
      } else {
        const list = /^( {0,3}(?:[-+*]|\d+[.)])(?=\s))/.exec(content);
        if (list) {
          const indent = list[1].match(/^ */)[0].length;
          fragment.append(content.slice(0, indent));
          fragment.append(span('list', list[1].slice(indent)));
          appendMarkdownLine(fragment, line.slice(list[1].length), content.slice(list[1].length));
        } else {
          appendMarkdownLine(fragment, line, content);
        }
      }
    }
    offset = end;
  }
  return fragment;
}
