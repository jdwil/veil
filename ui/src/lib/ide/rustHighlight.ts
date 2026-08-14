// Lightweight, dependency-free Rust syntax highlighter.
//
// Tokenizes into spans with a CSS class. Deliberately simple — enough to make
// generated Rust readable in the code-preview panel without pulling in a
// heavyweight highlighter. Not a full Rust grammar.

export interface Token {
  text: string;
  cls: string; // '' for plain text
}

const KEYWORDS = new Set([
  'as', 'async', 'await', 'break', 'const', 'continue', 'crate', 'dyn', 'else',
  'enum', 'extern', 'fn', 'for', 'if', 'impl', 'in', 'let', 'loop', 'match',
  'mod', 'move', 'mut', 'pub', 'ref', 'return', 'self', 'Self', 'static',
  'struct', 'super', 'trait', 'type', 'unsafe', 'use', 'where', 'while',
]);

// Common built-in / std types worth colouring distinctly.
const TYPES = new Set([
  'String', 'Vec', 'Option', 'Result', 'Box', 'Arc', 'Rc', 'HashMap',
  'HashSet', 'Uuid', 'DateTime', 'Utc', 'bool', 'i8', 'i16', 'i32', 'i64',
  'u8', 'u16', 'u32', 'u64', 'usize', 'isize', 'f32', 'f64', 'str', 'char',
]);

/** Tokenize a line of Rust into styled spans. */
export function highlightLine(line: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const n = line.length;

  const push = (text: string, cls: string) => {
    if (text) tokens.push({ text, cls });
  };

  while (i < n) {
    const c = line[i];

    // Line comment — rest of the line.
    if (c === '/' && line[i + 1] === '/') {
      push(line.slice(i), 'tok-comment');
      break;
    }

    // String literal (no multi-line handling; generated code stays on one line).
    if (c === '"') {
      let j = i + 1;
      while (j < n && line[j] !== '"') {
        if (line[j] === '\\') j++;
        j++;
      }
      push(line.slice(i, Math.min(j + 1, n)), 'tok-string');
      i = j + 1;
      continue;
    }

    // Char / lifetime — a single quote.
    if (c === "'") {
      let j = i + 1;
      while (j < n && /[A-Za-z0-9_]/.test(line[j])) j++;
      // char literal like 'a' vs lifetime 'a
      if (line[j] === "'") j++;
      push(line.slice(i, j), 'tok-lifetime');
      i = j;
      continue;
    }

    // Number.
    if (/[0-9]/.test(c)) {
      let j = i;
      while (j < n && /[0-9._]/.test(line[j])) j++;
      push(line.slice(i, j), 'tok-number');
      i = j;
      continue;
    }

    // Identifier / keyword / type.
    if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < n && /[A-Za-z0-9_]/.test(line[j])) j++;
      const word = line.slice(i, j);
      // A `word(` is a function/macro call; `word!` is a macro.
      const next = line[j];
      let cls = '';
      if (KEYWORDS.has(word)) cls = 'tok-keyword';
      else if (TYPES.has(word)) cls = 'tok-type';
      else if (next === '!') cls = 'tok-macro';
      else if (next === '(') cls = 'tok-fn';
      else if (/^[A-Z]/.test(word)) cls = 'tok-type';
      push(word, cls);
      i = j;
      continue;
    }

    // Attribute #[...] — colour the leading punctuation subtly.
    if (c === '#' && line[i + 1] === '[') {
      let j = i + 2;
      let depth = 1;
      while (j < n && depth > 0) {
        if (line[j] === '[') depth++;
        else if (line[j] === ']') depth--;
        j++;
      }
      push(line.slice(i, j), 'tok-attr');
      i = j;
      continue;
    }

    // Punctuation and everything else — pass through as plain.
    push(c, '');
    i++;
  }

  return tokens;
}
