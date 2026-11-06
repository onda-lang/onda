// Collection provenance stays symbolic until an operation demands a slot range.
export function selectedResourceRange(selector, len, bounds) {
  if (!Number.isInteger(len) || len <= 0) return [0, 0];
  if (selector?.kind === "constant" && selector.data?.type === "i32"
      && Number.isInteger(selector.data.value)) {
    let slot = selector.data.value;
    if (bounds === "clamp") slot = Math.min(len - 1, Math.max(0, slot));
    if (slot >= 0 && slot < len) return [slot, slot + 1];
  }
  return [0, len];
}

export class CollectionOrigins {
  constructor(maps, origins) {
    this.maps = maps ?? [];
    this.origins = origins;
    this.nodes = new Map();
  }

  add(id, len, source) {
    if (!this.nodes.has(id)) this.nodes.set(id, { len, sources: [] });
    if (source) this.nodes.get(id).sources.push(source);
  }

  has(id) { return this.nodes.has(id); }

  slot(id, slot, scalarOrigins) {
    const len = this.nodes.get(id)?.len ?? 0;
    return Number.isInteger(slot) && slot >= 0 && slot < len
      ? this.range(id, slot, slot + 1, scalarOrigins)
      : this.range(id, 0, len, scalarOrigins);
  }

  range(id, start, end, scalarOrigins) {
    const result = new Set();
    const pending = [[id, start, end]];
    const visited = new Set();
    const emit = (origin, offset, start, end) => {
      for (let index = start; index < end; index += 1) result.add(origin(offset + index));
    };
    while (pending.length) {
      let [id, start, end] = pending.pop();
      const key = `${id}:${start}:${end}`;
      if (start >= end || visited.has(key)) continue;
      visited.add(key);
      const node = this.nodes.get(id);
      if (!node) {
        for (const origin of scalarOrigins(id)) result.add(origin);
        continue;
      }
      end = Math.min(end, node.len);
      for (const source of node.sources) {
        if (source.kind === "value" && source.data.kind === "local") {
          pending.push([source.data.data, start, end]);
        } else if (source.kind === "place") {
          const base = source.data.base;
          if (base.kind === "local") pending.push([base.data, start, end]);
          else if (base.kind === "parameter") {
            emit((slot) => this.origins.parameter(base.data, slot), 0, start, end);
          }
        } else if (source.kind === "buffer_span") {
          const { kind, data } = source.data;
          if (kind === "interface") emit(this.origins.buffer, data.first, start, end);
          else if (kind === "parameter") {
            emit((slot) => this.origins.parameter(data.span, slot), data.start, start, end);
          } else if (kind === "mapped_interface") {
            const entries = this.maps[data.map]?.buffers ?? [];
            for (let index = start; index < end; index += 1) {
              const buffer = entries[data.start + index];
              if (buffer !== undefined) result.add(this.origins.buffer(buffer));
            }
          } else if (kind === "local") {
            const sourceLen = this.nodes.get(data.collection)?.len ?? 0;
            const [first, last] = selectedResourceRange(data.start, sourceLen - data.len + 1, data.bounds);
            if (first < last) pending.push([data.collection, first + start, last - 1 + end]);
          }
        }
      }
    }
    return result;
  }
}
