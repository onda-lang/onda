// Stable identity for the browser artifact cache. Editor tab order is UI state,
// so source paths are sorted before comparing projects.
export function compilationKey(project, options) {
  const sources = Object.entries(project.sources ?? {})
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0));
  return JSON.stringify({
    entry: project.entry,
    sources,
    constants: Object.entries(project.constants ?? {}).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0),
    sampleRate: Number(options.sampleRate),
    blockSize: Number(options.blockSize),
    fastMath: options.codegen?.fastMath === true,
  });
}
