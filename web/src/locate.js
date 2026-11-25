// Where in the text of a model a diagnostic points: its `path` is either
// "line 14 column 4" (the text is not a model) or a path into the document
// such as "components[1].resistance_ohm".

/// The range {start, end} of the text that `path` names, or null.
export function locate(text, path) {
  if (typeof path !== "string" || path === "") return null;
  const lineColumn = /^line (\d+) column (\d+)$/.exec(path);
  if (lineColumn) return lineColumnRange(text, Number(lineColumn[1]), Number(lineColumn[2]));
  const steps = [];
  for (const m of path.matchAll(/([^.[\]]+)|\[(\d+)\]/g)) {
    steps.push(m[2] !== undefined ? Number(m[2]) : m[1]);
  }
  if (steps.length === 0) return null;
  try {
    return walk(text, steps);
  } catch {
    return null; // The text is not JSON any more: nothing to point at.
  }
}

/// The ranges {start, end} of the items of the list `key` at the top of
/// the document ("components"), in their order; empty when the text is not
/// JSON or has no such list.
export function itemRanges(text, key) {
  const ranges = [];
  try {
    const top = skipSpace(text, 0);
    if (text[top] !== "{") return ranges;
    forEachMember(text, top, (name, start) => {
      if (name !== key) return true;
      if (text[start] === "[") {
        forEachItem(text, start, (from, to) => {
          ranges.push({ start: from, end: to });
          return true;
        });
      }
      return false;
    });
  } catch {
    return []; // The text is not JSON: nothing to point at.
  }
  return ranges;
}

const encoder = new TextEncoder();

function lineColumnRange(text, line, column) {
  if (line < 1) return null;
  let start = 0;
  for (let n = 1; n < line; n += 1) {
    const next = text.indexOf("\n", start);
    if (next < 0) return null;
    start = next + 1;
  }
  let lineEnd = text.indexOf("\n", start);
  if (lineEnd < 0) lineEnd = text.length;
  const lineText = text.slice(start, lineEnd);
  let colOffset = Math.max(0, column - 1);
  // The parser reports a UTF-8 byte column; convert to UTF-16 code units when non-ASCII.
  if (/[^\x00-\x7f]/.test(lineText)) {
    let bytes = 0;
    let i = 0;
    while (i < lineText.length && bytes < colOffset) {
      const cp = lineText.codePointAt(i);
      const step = cp > 0xffff ? 2 : 1;
      bytes += encoder.encode(lineText.slice(i, i + step)).length;
      i += step;
    }
    colOffset = i;
  }
  // The parser reports the place where it gave up; select the line up to it
  // (or the whole line when that is its beginning).
  const at = Math.min(lineEnd, start + colOffset);
  return at > start ? { start, end: Math.min(lineEnd, at + 1) } : { start, end: lineEnd };
}

/// Follows `steps` (keys and indexes) through the JSON text. As far as the
/// path can be followed: a diagnostic may name a field that is missing, and
/// then the object that should have it is the place.
function walk(text, steps) {
  let pos = skipSpace(text, 0);
  let range = { start: pos, end: valueEnd(text, pos) };
  for (const step of steps) {
    pos = range.start;
    let found = null;
    if (typeof step === "number" && text[pos] === "[") {
      let index = 0;
      forEachItem(text, pos, (start, end) => {
        if (index === step) found = { start, end };
        index += 1;
        return found === null;
      });
    } else if (typeof step === "string" && text[pos] === "{") {
      forEachMember(text, pos, (key, start, end) => {
        if (key === step) found = { start, end };
        return found === null;
      });
    }
    if (!found) break;
    range = found;
  }
  return range;
}

function skipSpace(text, pos) {
  while (pos < text.length && /\s/.test(text[pos])) pos += 1;
  return pos;
}

function stringEnd(text, pos) {
  for (let i = pos + 1; i < text.length; i += 1) {
    if (text[i] === "\\") i += 1;
    else if (text[i] === '"') return i + 1;
  }
  throw new Error("unterminated string");
}

/// The position after the value that starts at `pos`.
function valueEnd(text, pos) {
  const c = text[pos];
  if (c === '"') return stringEnd(text, pos);
  if (c === "{" || c === "[") {
    let depth = 0;
    for (let i = pos; i < text.length; i += 1) {
      const d = text[i];
      if (d === '"') i = stringEnd(text, i) - 1;
      else if (d === "{" || d === "[") depth += 1;
      else if (d === "}" || d === "]") {
        depth -= 1;
        if (depth === 0) return i + 1;
      }
    }
    throw new Error("unterminated value");
  }
  let i = pos;
  while (i < text.length && !/[\s,\]}]/.test(text[i])) i += 1;
  if (i === pos) throw new Error("no value");
  return i;
}

function forEachItem(text, pos, visit) {
  let i = skipSpace(text, pos + 1);
  while (i < text.length && text[i] !== "]") {
    const end = valueEnd(text, i);
    if (!visit(i, end)) return;
    i = skipSpace(text, end);
    if (text[i] === ",") i = skipSpace(text, i + 1);
  }
}

function forEachMember(text, pos, visit) {
  let i = skipSpace(text, pos + 1);
  while (i < text.length && text[i] !== "}") {
    if (text[i] !== '"') throw new Error("no key");
    const keyEnd = stringEnd(text, i);
    const key = JSON.parse(text.slice(i, keyEnd));
    i = skipSpace(text, keyEnd);
    if (text[i] !== ":") throw new Error("no colon");
    i = skipSpace(text, i + 1);
    const end = valueEnd(text, i);
    if (!visit(key, i, end)) return;
    i = skipSpace(text, end);
    if (text[i] === ",") i = skipSpace(text, i + 1);
  }
}
