// Shared by the adapter and worklet; keep this module independent of package
// imports so worklets also load from a plain static HTTP directory.
export function paramAddress(params, selector) {
  const direct = Number.isInteger(selector) ? params[selector] : params.find((param) => param.name === selector);
  if (direct) return { info: direct, element: null };
  if (typeof selector === "string") {
    const match = /^(.*)\[(0|[1-9][0-9]*)\]$/.exec(selector);
    const root = match && params.find((param) => param.name === match[1] && param.type_repr !== param.scalar);
    if (root) {
      const element = Number(match[2]);
      if (!Number.isSafeInteger(element) || element >= root.array_len) {
        throw new Error(`Onda parameter '${root.name}' element index is out of bounds`);
      }
      return { info: root, element };
    }
  }
  throw new Error(`unknown Onda parameter '${String(selector)}'`);
}
