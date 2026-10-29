// Capture only the display's min/max budget; no raw float arrays cross the iframe.
const CAPTURE_INTERVAL_MS = 1000 / 30;

export class BrowserScopeSource {
  constructor(runView) {
    this.runView = runView;
    this.configuration = { columns: 512, visible: true };
    this.timer = 0;
    this.analysers = [];
    this.samples = [];
    this.packet = new Uint8Array(8);
    this.onVisibility = () => this.schedule();
    document.addEventListener("visibilitychange", this.onVisibility);
  }

  configure({ columns, visible }) {
    if (!Number.isInteger(columns) || columns < 1 || columns > 512) return;
    this.configuration = { columns, visible: visible !== false };
    this.schedule();
  }

  start(context, node, channels) {
    this.stop();
    if (!channels) return;
    this.splitter = context.createChannelSplitter(channels);
    this.silentGain = context.createGain();
    this.silentGain.gain.value = 0;
    node.connect(this.splitter);
    for (let channel = 0; channel < channels; channel++) {
      const analyser = context.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0;
      this.splitter.connect(analyser, channel, 0);
      analyser.connect(this.silentGain);
      this.analysers.push(analyser);
      this.samples.push(new Float32Array(analyser.fftSize));
    }
    this.silentGain.connect(context.destination);
    this.schedule();
  }

  schedule() {
    if (
      !this.analysers.length ||
      !this.configuration.visible ||
      document.hidden
    ) {
      if (this.timer) clearInterval(this.timer);
      this.timer = 0;
      return;
    }
    if (!this.timer)
      this.timer = setInterval(() => this.capture(), CAPTURE_INTERVAL_MS);
  }

  capture() {
    if (
      !this.analysers.length ||
      !this.configuration.visible ||
      document.hidden
    )
      return;
    for (let i = 0; i < this.analysers.length; i++) {
      this.analysers[i].getFloatTimeDomainData(this.samples[i]);
    }
    this.packet = scopePacket(
      this.samples,
      this.configuration.columns,
      this.packet,
    );
    this.runView.postScopePacket(this.packet);
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = 0;
    this.splitter?.disconnect();
    this.silentGain?.disconnect();
    for (const analyser of this.analysers) analyser.disconnect();
    this.splitter = null;
    this.silentGain = null;
    this.analysers = [];
    this.samples = [];
    this.runView.postScope(0, []);
  }

  dispose() {
    this.stop();
    document.removeEventListener("visibilitychange", this.onVisibility);
  }
}

export function scopePacket(samples, budget, reuse = new Uint8Array()) {
  const channels = Math.min(samples.length, 64);
  const frames = channels ? samples[0].length : 0;
  const columns = channels
    ? Math.min(frames, Math.max(1, Math.min(512, budget)))
    : 0;
  const length = 8 + channels * columns * 4;
  const packet = reuse.length === length ? reuse : new Uint8Array(length);
  const view = new DataView(
    packet.buffer,
    packet.byteOffset,
    packet.byteLength,
  );
  packet.set([79, 83, 81, 49]);
  view.setUint16(4, channels, true);
  view.setUint16(6, columns, true);
  for (let channel = 0; channel < channels; channel++) {
    for (let column = 0; column < columns; column++) {
      let lo = 32767,
        hi = -32767;
      for (
        let frame = Math.floor((column * frames) / columns);
        frame < Math.floor(((column + 1) * frames) / columns);
        frame++
      ) {
        const sample = samples[channel][frame];
        const scaled =
          Math.max(-1, Math.min(1, Number.isFinite(sample) ? sample : 0)) *
          32767;
        const value = scaled < 0 ? -Math.round(-scaled) : Math.round(scaled);
        lo = Math.min(lo, value);
        hi = Math.max(hi, value);
      }
      const offset = 8 + (channel * columns + column) * 4;
      view.setInt16(offset, lo, true);
      view.setInt16(offset + 2, hi, true);
    }
  }
  return packet;
}
