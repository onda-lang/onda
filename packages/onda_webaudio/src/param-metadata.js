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

export function paramElementAddress(params, selector, element) {
  const address = paramAddress(params, selector);
  if (address.element !== null) {
    throw new Error("an explicit element write requires a parameter name or index");
  }
  if (
    !Number.isSafeInteger(element)
    || element < 0
    || element >= address.info.array_len
  ) {
    throw new Error(`Onda parameter '${address.info.name}' element index is out of bounds`);
  }
  return { info: address.info, element };
}

export function paramSmoothingSamples(seconds, sampleRate, blockSize) {
  const samples = Math.ceil(seconds * sampleRate);
  if (!Number.isFinite(seconds) || seconds < 0 || !Number.isSafeInteger(samples)) {
    throw new Error("paramSmoothingSeconds must be finite, non-negative, and fit the host sample counter");
  }
  return samples > blockSize ? samples : 0;
}

// Private host state: descriptors and processor snapshots contain no ramps.
export class ParamSmoothing {
  constructor(params, samples, view, paramsPtr) {
    this.samples = samples;
    this.inverseSamples = 1 / Math.max(samples, 1);
    this.started = false;
    this.addresses = new Map();
    this.entries = [];
    if (samples > 0) {
      for (const param of params) {
        if ((param.scalar !== "f32" && param.scalar !== "f64")
          || param.param_control?.step_count != null) continue;
        const elements = [];
        const f32 = param.scalar === "f32";
        for (let element = 0; element < param.array_len; element += 1) {
          const offset = Number(param.byte_offset) + element * (f32 ? 4 : 8);
          const value = f32 ? view.getFloat32(paramsPtr + offset, true)
            : view.getFloat64(paramsPtr + offset, true);
          elements.push(this.entries.length);
          this.entries.push({ offset, f32, current: value, start: value,
            target: value, elapsed: samples, active: false });
        }
        this.addresses.set(param, elements);
      }
    }
    this.active = new Uint32Array(this.entries.length);
    this.activeCount = 0;
  }

  isEnabledFor(param) {
    return this.addresses.has(param);
  }

  setTarget(param, element, value, initialized) {
    const id = this.addresses.get(param)?.[element];
    if (id === undefined) return false;
    const entry = this.entries[id];
    const target = entry.f32 ? Math.fround(Number(value)) : Number(value);
    if (!initialized || !this.started || !Number.isFinite(target) || !Number.isFinite(entry.current)) {
      entry.current = target;
      entry.target = target;
      entry.elapsed = this.samples;
      return false;
    }
    if (target === entry.target) return true;
    entry.start = entry.current;
    entry.target = target;
    entry.elapsed = target === entry.current ? this.samples : 0;
    if (!entry.active) {
      entry.active = true;
      this.active[this.activeCount++] = id;
    }
    return true;
  }

  beginBlock(frames, view, paramsPtr) {
    this.started = true;
    let cursor = 0;
    while (cursor < this.activeCount) {
      const entry = this.entries[this.active[cursor]];
      entry.elapsed += Math.min(frames, this.samples - entry.elapsed);
      let value = entry.target;
      if (entry.elapsed !== this.samples) {
        const weight = entry.elapsed * this.inverseSamples;
        if ((entry.start < 0) !== (entry.target < 0)) {
          value = entry.start * (1 - weight) + entry.target * weight;
        } else if (weight <= 0.5) {
          value = entry.start + (entry.target - entry.start) * weight;
        } else {
          value = entry.target + (entry.start - entry.target) * (1 - weight);
        }
      }
      this.write(entry, value, view, paramsPtr);
      if (entry.elapsed === this.samples) {
        entry.active = false;
        this.active[cursor] = this.active[--this.activeCount];
      } else {
        cursor += 1;
      }
    }
  }

  settle(view, paramsPtr) {
    this.activeCount = 0;
    this.started = false;
    for (const entry of this.entries) {
      this.write(entry, entry.target, view, paramsPtr);
      entry.start = entry.target;
      entry.elapsed = this.samples;
      entry.active = false;
    }
  }

  reset(view, paramsPtr) {
    for (const entry of this.entries) {
      entry.target = entry.f32 ? view.getFloat32(paramsPtr + entry.offset, true)
        : view.getFloat64(paramsPtr + entry.offset, true);
    }
    this.settle(view, paramsPtr);
  }

  write(entry, value, view, paramsPtr) {
    entry.current = entry.f32 ? Math.fround(value) : value;
    if (entry.f32) view.setFloat32(paramsPtr + entry.offset, entry.current, true);
    else view.setFloat64(paramsPtr + entry.offset, entry.current, true);
  }
}
