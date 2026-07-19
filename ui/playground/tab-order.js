export function reorderMap(map, orderedKeys) {
  const reordered = new Map();
  for (const key of orderedKeys) {
    if (map.has(key) && !reordered.has(key)) reordered.set(key, map.get(key));
  }
  for (const [key, value] of map) {
    if (!reordered.has(key)) reordered.set(key, value);
  }
  return reordered;
}
