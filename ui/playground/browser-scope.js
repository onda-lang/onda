import { registerMeter } from "./meter-worklet.js";

const CAPTURE_INTERVAL_MS = 1000 / 30;

export class BrowserScopeSource {
  constructor(runView) {
    this.runView = runView;
    this.configuration = { columns: 512, visible: false, meter: false };
    this.generation = 0;
    this.timer = 0;
    this.analysers = [];
    this.samples = [];
    this.packet = new Uint8Array(8);
    this.onVisibility = () => this.schedule();
    document.addEventListener("visibilitychange", this.onVisibility);
  }

  configure({ columns, visible, meter }) {
    if (!Number.isInteger(columns) || columns < 1 || columns > 512) return;
    this.configuration = { columns, visible: visible === true, meter: meter === true };
    this.schedule();
  }

  async start(context, node, channels) {
    this.stop();
    if (!channels) return;
    const generation = this.generation;
    await registerMeter(context);
    if (generation !== this.generation) return;
    this.context = context;
    this.source = node;
    this.channels = channels;
    this.meter = new AudioWorkletNode(context, "onda-output-meter", {
      numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
      channelCount: channels, channelCountMode: "explicit", channelInterpretation: "discrete",
    });
    this.meter.port.onmessage = ({ data }) => {
      if (generation === this.generation && !document.hidden &&
          (this.configuration.meter || this.configuration.visible))
        this.runView.postMeter(data.channels, data.peaks);
    };
    // The worklet emits silence; connecting it keeps the audio graph pulling input.
    node.connect(this.meter);
    this.meter.connect(context.destination);
    this.schedule();
  }

  connectWaveform() {
    this.splitter = this.context.createChannelSplitter(this.channels);
    this.silentGain = this.context.createGain();
    this.silentGain.gain.value = 0;
    this.source.connect(this.splitter);
    for (let channel = 0; channel < this.channels; channel++) {
      const analyser = this.context.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0;
      this.splitter.connect(analyser, channel, 0);
      analyser.connect(this.silentGain);
      this.analysers.push(analyser);
      this.samples.push(new Float32Array(analyser.fftSize));
    }
    this.silentGain.connect(this.context.destination);
  }

  disconnectWaveform() {
    if (this.timer) clearInterval(this.timer);
    this.timer = 0;
    if (this.splitter) this.source.disconnect(this.splitter);
    this.splitter?.disconnect();
    this.silentGain?.disconnect();
    for (const analyser of this.analysers) analyser.disconnect();
    this.splitter = null;
    this.silentGain = null;
    this.analysers = [];
    this.samples = [];
  }

  schedule() {
    const waveform = this.configuration.visible && !document.hidden;
    const metering = (waveform || this.configuration.meter) && !document.hidden;
    if (this.meter && this.metering !== metering) {
      this.meter.port.postMessage(metering);
      this.metering = metering;
    }
    if (!waveform) this.disconnectWaveform();
    else {
      if (this.context && !this.analysers.length) this.connectWaveform();
      if (this.analysers.length && !this.timer)
        this.timer = setInterval(() => this.capture(), CAPTURE_INTERVAL_MS);
    }
  }

  capture() {
    if (!this.analysers.length || !this.configuration.visible || document.hidden) return;
    for (let i = 0; i < this.analysers.length; i++)
      this.analysers[i].getFloatTimeDomainData(this.samples[i]);
    this.packet = scopePacket(this.samples, this.configuration.columns, this.packet);
    this.runView.postScopePacket(this.packet);
  }

  stop() {
    ++this.generation;
    this.disconnectWaveform();
    if (this.meter) {
      this.source.disconnect(this.meter);
      this.meter.disconnect();
      this.meter.port.postMessage("stop");
      this.meter.port.onmessage = null;
      this.meter.port.close();
    }
    this.meter = null;
    this.metering = undefined;
    this.source = null;
    this.context = null;
    this.runView.postScope(0, []);
    this.runView.postMeter(0, []);
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
