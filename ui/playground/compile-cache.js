// Stable identity for the browser artifact cache. Editor tab order is UI state,
// so source paths are sorted before comparing projects.
export function compilationKey(project, options) {
  const sources = Object.entries(project.sources ?? {})
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
  return JSON.stringify({
    entry: project.entry,
    sources,
    sampleRate: Number(options.sampleRate),
    blockSize: Number(options.blockSize),
  });
}
