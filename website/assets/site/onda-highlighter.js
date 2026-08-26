const sectionWords = new Set([
  "ins", "inputs", "outs", "outputs", "params", "kins", "kouts",
  "buffers", "events", "init", "block", "sample", "graph",
]);
const declarationWords = new Set([
  "const", "def", "event", "proc", "processor", "struct", "namespace",
]);
const nameFollowing = new Set([
  "def", "event", "proc", "processor", "struct", "namespace",
]);
const keywordWords = new Set([
  "if", "elif", "else", "for", "in", "while", "loop", "break",
  "continue", "return", "assert", "import", "include", "use", "pub",
  "as", "private", "pin", "config",
]);
const typeWords = new Set(["f32", "f64", "i32", "i64", "bool", "buffer"]);
const constantWords = new Set([
  "true", "false", "PI", "TWO_PI", "TWOPI", "SR", "SAMPLE_RATE",
  "SAMPLERATE", "HOST_SR", "HOST_SAMPLE_RATE", "HOST_SAMPLERATE",
  "BS", "BLOCK_SIZE", "BLOCKSIZE",
]);

export function highlightOnda(code, source) {
  source = typeof source === "string" ? source : code.textContent || "";
  const fragment = document.createDocumentFragment();
  let offset = 0;
  let lineStart = true;
  let expectedName = false;

  const append = (value, className) => {
    if (!className) {
      fragment.append(document.createTextNode(value));
      return;
    }
    const span = document.createElement("span");
    span.className = className;
    span.textContent = value;
    fragment.append(span);
  };

  while (offset < source.length) {
    const rest = source.slice(offset);
    let match;

    if ((match = rest.match(/^\s+/))) {
      append(match[0]);
      if (match[0].includes("\n")) lineStart = true;
      offset += match[0].length;
      continue;
    }
    if (rest[0] === "#") {
      const end = rest.indexOf("\n");
      const value = end < 0 ? rest : rest.slice(0, end);
      append(value, "syntax-comment");
      offset += value.length;
      continue;
    }
    if (rest[0] === '"') {
      match = rest.match(/^"(?:\\.|[^"\\])*"?/);
      append(match[0], "syntax-string");
      offset += match[0].length;
      lineStart = false;
      continue;
    }
    if ((match = rest.match(/^\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/))) {
      append(match[0], "syntax-number");
      offset += match[0].length;
      lineStart = false;
      continue;
    }
    if ((match = rest.match(/^[A-Za-z_][A-Za-z0-9_]*/))) {
      const word = match[0];
      let className = "";
      if (lineStart && sectionWords.has(word)) className = "syntax-section";
      else if (declarationWords.has(word)) className = "syntax-declaration";
      else if (expectedName) className = "syntax-function";
      else if (keywordWords.has(word)) className = "syntax-keyword";
      else if (typeWords.has(word)) className = "syntax-type";
      else if (constantWords.has(word)) className = "syntax-constant";
      else if (/^(?:in|out|kout|param|kin|buf)\d+$/.test(word)) className = "syntax-constant";
      else if (/^\s*\(/.test(rest.slice(word.length))) className = "syntax-function";
      append(word, className);
      expectedName = nameFollowing.has(word);
      offset += word.length;
      lineStart = false;
      continue;
    }
    if ((match = rest.match(/^(?:>>\[[^\]\n]+\]|<<\[[^\]\n]+\]|\.\.=|\.\.|>>|<<|==|!=|<=|>=|&&|\|\||::|@(?:sample|block)|[+\-*/%=&|^~!<>])/))) {
      append(match[0], "syntax-operator");
      offset += match[0].length;
      lineStart = false;
      continue;
    }

    append(rest[0]);
    offset += 1;
    lineStart = false;
  }

  code.replaceChildren(fragment);
}
