// Canonical JSON across the Wasm boundary and saved presentation state.
export function computerKey(event) {
  return /^Key[A-Z]$/.test(event.code) ? event.code.slice(3).toLowerCase() : event.key;
}

export function wireJson(value) {
  return JSON.stringify(value, (_, item) => {
    if (typeof item !== "number") return item;
    if (Object.is(item, -0)) return "-0";
    return Number.isFinite(item) ? item : String(item);
  });
}
