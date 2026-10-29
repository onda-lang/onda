import { Direction, EditorView, RectangleMarker, layer } from "@codemirror/view";

function selectionMarkers(view) {
  const ranges = view.state.selection.ranges.filter(range => !range.empty);
  if (!ranges.length) return [];

  const scroll = view.scrollDOM.getBoundingClientRect();
  const originLeft = (view.textDirection === Direction.LTR
    ? scroll.left : scroll.right - view.scrollDOM.clientWidth * view.scaleX)
    - view.scrollDOM.scrollLeft * view.scaleX;
  const originTop = scroll.top - view.scrollDOM.scrollTop * view.scaleY;
  const content = view.contentDOM.getBoundingClientRect();
  const line = view.contentDOM.querySelector(".cm-line");
  if (!line) return [];
  const style = view.dom.ownerDocument.defaultView.getComputedStyle(line);
  const left = content.left - originLeft;
  const right = content.right - originLeft;
  const textLeft = left + parseFloat(style.paddingLeft) + Math.min(0, parseFloat(style.textIndent));
  const textRight = right - parseFloat(style.paddingRight);
  const documentTop = view.documentTop - originTop;

  // The editor keeps lines unwrapped. Snap glyph edges to their nearest row
  // boundary, including the joins between a selection's end rows and middle.
  const rowEdge = y => {
    const height = y - documentTop;
    const { top, bottom } = view.lineBlockAtHeight(height);
    return documentTop + (height - top < bottom - height ? top : bottom);
  };
  return ranges.flatMap(range => RectangleMarker.forRange(view, "cm-selectionBackground", range)
    .filter(marker => marker.width > 0)
    .map(marker => {
      const top = rowEdge(marker.top);
      const bottom = rowEdge(marker.top + marker.height);
      const from = Math.abs(marker.left - textLeft) < .5 ? left : marker.left;
      const to = Math.abs(marker.left + marker.width - textRight) < .5
        ? right : marker.left + marker.width;
      return new RectangleMarker("cm-selectionBackground", from, top, to - from, bottom - top);
    }));
}

export const editorSelection = [
  layer({
    above: false,
    class: "cm-onda-selectionLayer",
    markers: selectionMarkers,
    update: update => update.docChanged || update.selectionSet || update.viewportChanged,
  }),
  EditorView.editorAttributes.of(view => ({
    class: view.state.selection.ranges.some(range => !range.empty) ? "cm-has-selection" : "",
  })),
  EditorView.theme({
    // Keep CodeMirror's cursors and touch handles; replace only its backgrounds.
    ".cm-selectionLayer .cm-selectionBackground": { visibility: "hidden" },
    ".cm-onda-selectionLayer .cm-selectionBackground": {
      backgroundColor: "color-mix(in srgb, var(--ink) 24%, transparent) !important",
    },
    "&.cm-has-selection .cm-activeLine, &.cm-has-selection .cm-activeLineGutter": {
      backgroundColor: "transparent",
    },
  }),
];
