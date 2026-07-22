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

// Use one SVG cube for every completion kind. This keeps the list quiet and
// predictable without relying on mathematical glyphs from the editor font.
const completionCubeMask = iconMask(
  "<path d='M8 1.75 14 5v6L8 14.25 2 11V5zM2 5l6 3.25L14 5M8 8.25v6'/>",
);

export const completionIconMasks = Object.freeze(Object.fromEntries(
  [...new Set(["text", ...COMPLETION_TYPES.values()])]
    .map((type) => [type, completionCubeMask]),
));
