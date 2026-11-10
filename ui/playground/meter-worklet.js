// Kept as a module string so every playground bundle carries its audio-thread meter.
export const meterWorklet = `
class OndaMeter extends AudioWorkletProcessor {
  constructor() {
    super();
    this.enabled = false;
    this.stopped = false;
    this.frames = 0;
    this.peaks = new Float32Array(64);
    this.port.onmessage = ({ data }) => {
      this.stopped = data === "stop";
      this.enabled = data === true;
      this.frames = 0;
      this.peaks.fill(0);
    };
  }
  process(inputs) {
    if (this.stopped) return false;
    const input = inputs[0];
    if (!this.enabled || !input?.length) return true;
    const count = Math.min(input.length, this.peaks.length);
    for (let channel = 0; channel < count; channel++) {
      let peak = this.peaks[channel];
      const samples = input[channel];
      for (let frame = 0; frame < samples.length; frame++) {
        const magnitude = Math.abs(samples[frame]);
        if (Number.isFinite(magnitude) && magnitude > peak) peak = magnitude;
      }
      this.peaks[channel] = peak;
    }
    this.frames += input[0].length;
    if (this.frames >= sampleRate / 30) {
      this.port.postMessage({ channels: input.length, peaks: Array.from(this.peaks.subarray(0, count)) });
      this.peaks.fill(0);
      this.frames %= sampleRate / 30;
    }
    return true;
  }
}
registerProcessor("onda-output-meter", OndaMeter);
`;

const registrations = new WeakMap();
export function registerMeter(context) {
  if (!registrations.has(context)) {
    const url = URL.createObjectURL(new Blob([meterWorklet], { type: "text/javascript" }));
    const registration = context.audioWorklet.addModule(url).finally(() => URL.revokeObjectURL(url));
    registrations.set(context, registration);
    registration.catch(() => registrations.delete(context));
  }
  return registrations.get(context);
}
