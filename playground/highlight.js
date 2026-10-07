// Small Rust and Python highlighters for the generated code and the pasted
// source. They build text nodes and spans, never HTML, so docstrings from
// Python cannot inject markup.

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
