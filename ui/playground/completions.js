const COMPLETION_TYPES = new Map([
  [2, "method"],
  [3, "function"],
  [4, "constructor"],
  [5, "field"],
  [6, "variable"],
  [9, "namespace"],
  [10, "property"],
  [14, "keyword"],
  [17, "file"],
  [21, "constant"],
  [22, "struct"],
  [23, "event"],
  [25, "type"],
]);

export function completionType(kind) {
  return COMPLETION_TYPES.get(kind) ?? "text";
}

function iconMask(body) {
  const svg = [
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'",
    " fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'>",
    body,
    "</svg>",
  ].join("");
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

// These masks deliberately contain paths rather than font glyphs. CodeMirror's
// defaults use mathematical Unicode characters that are absent from some mono
// fonts, leaving otherwise valid LSP completion kinds visually blank.
export const completionIconMasks = Object.freeze({
  text: iconMask("<path d='M2.5 4h11M2.5 8h9M2.5 12h7'/>"),
  method: iconMask("<circle cx='8' cy='8' r='5.5'/><path d='M5.5 8h5M8 5.5v5'/>"),
  function: iconMask("<path d='M3 3.5h4M5.5 2v9.5c0 1.4-.8 2.2-2.5 2.5M3 7h4M10 6l3 2-3 2'/>"),
  constructor: iconMask("<path d='M8 1.75 14 5v6L8 14.25 2 11V5zM2 5l6 3.25L14 5M8 8.25v6'/>"),
  field: iconMask("<path d='M2.5 4h11M2.5 8h7M2.5 12h9'/><circle cx='12.5' cy='8' r='.75' fill='black' stroke='none'/>"),
  variable: iconMask("<path d='M3 3.5h2.5L10.5 12H13M13 3.5h-2.5L5.5 12H3'/>"),
  namespace: iconMask("<path d='M1.75 4h4l1.25 1.5h7.25v7.25H1.75z'/>"),
  property: iconMask("<path d='M2 4h12M2 8h12M2 12h12'/><circle cx='5' cy='4' r='1.25' fill='black' stroke='none'/><circle cx='11' cy='8' r='1.25' fill='black' stroke='none'/><circle cx='7' cy='12' r='1.25' fill='black' stroke='none'/>"),
  keyword: iconMask("<circle cx='5.25' cy='7' r='3.25'/><path d='M8.5 7H14M11.5 7v2M13.5 7v2'/>"),
  file: iconMask("<path d='M3 1.75h6l4 4v8.5H3zM9 1.75v4h4M5.5 9h5M5.5 11.5h4'/>"),
  constant: iconMask("<path d='m8 1.75 6.25 6.25L8 14.25 1.75 8zM5.5 8h5'/>"),
  struct: iconMask("<rect x='2' y='2' width='5' height='5'/><rect x='9' y='2' width='5' height='5'/><rect x='2' y='9' width='5' height='5'/><rect x='9' y='9' width='5' height='5'/>"),
  event: iconMask("<path d='m9.25 1.5-6 7.5h4l-.5 5.5 6-8h-4z'/>"),
  type: iconMask("<path d='M2.5 3h11M8 3v10M5.5 13h5'/>"),
});
