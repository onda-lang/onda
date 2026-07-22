// Shared browser adapter for the transport-neutral Onda run view.
import { flattenedAudioChannelCount } from "@onda-lang/webaudio";
import { UNBOUND_BUFFERS_MESSAGE } from "./browser-buffers.js";

export class BrowserRunViewHost {
  constructor(iframe, handlers = {}) {
    this.iframe = iframe;
    this.targetOrigin = new URL(iframe.src, window.location.href).origin;
    this.handlers = handlers;
    this.ready = false;
    this.state = {
      running: false,
      connected: false,
      path: "",
      status: "Stopped",
      error: "",
      sourceDirty: false,
      outputChannels: 0,
      buffers: [],
      events: [],
      params: [],
      inputDevices: [],
      outputDevices: [],
      currentInputDevice: null,
      currentOutputDevice: null,
      supportsDeviceSelection: false,
      themeMode: document.documentElement.dataset.theme || "auto",
    };
    this.onWindowMessage = (event) => {
      if (
        event.source !== this.iframe.contentWindow
        || event.origin !== this.targetOrigin
        || !event.data?.__ondaRunWebview
      ) return;
      void this.handleMessage(event.data.message);
    };
    window.addEventListener("message", this.onWindowMessage);
    this.themeObserver = new MutationObserver(() => {
      this.setState({
        themeMode: document.documentElement.dataset.theme || "auto",
      });
    });
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    this.iframe.addEventListener("load", () => this.postState());
  }

  setPath(path) {
    this.setState({ path });
  }

  setState(update) {
    Object.assign(this.state, update);
    this.postState();
  }

  setCompiling(path) {
    this.setState({
      path,
      running: false,
      connected: false,
      status: "Compiling",
      error: "",
    });
  }

  setStarting(path) {
    this.setState({
      path,
      running: false,
      connected: false,
      status: "Starting",
      error: "",
    });
  }

  setArtifact(artifact, bufferFiles) {
    const metadata = artifact.metadata.metadata;
    this.state.params = mergeParams(metadata.params ?? [], this.state.params);
    this.state.events = mergeEvents(metadata.events ?? [], this.state.events);
    this.state.buffers = mergeBuffers(metadata.buffers ?? [], this.state.buffers, bufferFiles);
    this.state.outputChannels = flattenedAudioChannelCount(metadata.outputs);
    this.state.error = "";
    this.state.sourceDirty = false;
    if (!this.buffersReady()) {
      this.state.running = false;
      this.state.connected = false;
      this.state.status = UNBOUND_BUFFERS_MESSAGE;
    }
    this.postState();
  }

  buffersReady() {
    return this.state.buffers.every((buffer) => Boolean(buffer.loadedPath));
  }

  setWaitingForBuffers() {
    this.setState({
      running: false,
      connected: false,
      status: UNBOUND_BUFFERS_MESSAGE,
      error: "",
    });
    this.postScope(0, []);
  }

  clearArtifact(path = this.state.path) {
    this.setState({
      path,
      running: false,
      connected: false,
      status: "Stopped",
      error: "",
      sourceDirty: false,
      outputChannels: 0,
      buffers: [],
      events: [],
      params: [],
    });
    this.postScope(0, []);
  }

  markSourceDirty(path = this.state.path) {
    this.setState({ path, sourceDirty: true });
  }

  setRunning(sampleRate, status) {
    this.setState({
      running: true,
      connected: true,
      status: status ?? `Running at ${sampleRate.toLocaleString()} Hz`,
      error: "",
    });
  }

  setStopped(status = "Stopped") {
    this.setState({
      running: false,
      connected: false,
      status: this.buffersReady() ? status : UNBOUND_BUFFERS_MESSAGE,
    });
    this.postScope(0, []);
  }

  setError(error) {
    this.setState({
      running: false,
      connected: false,
      status: "Stopped",
      error: String(error?.message ?? error),
    });
  }

  showError(error) {
    this.setState({ error: String(error?.message ?? error) });
  }

  resetValues() {
    this.state.params = this.state.params.map((param) => ({
      ...param,
      value: initialParamValue(param),
    }));
    this.state.events = this.state.events.map((event) => ({
      ...event,
      args: event.args.map((arg) => ({ ...arg, value: initialEventArgValue(arg) })),
    }));
    this.postState();
  }

  updateBufferFile(name, file) {
    this.state.buffers = this.state.buffers.map((buffer) =>
      buffer.name === name ? { ...buffer, loadedPath: file?.name ?? null } : buffer
    );
    this.state.error = "";
    if (!this.buffersReady()) {
      this.state.running = false;
      this.state.connected = false;
      this.state.status = UNBOUND_BUFFERS_MESSAGE;
    } else if (this.state.status === UNBOUND_BUFFERS_MESSAGE) {
      this.state.status = "Stopped";
    }
    this.postState();
  }

  postState() {
    this.post({ type: "state", state: this.state });
  }

  postScope(channels, samples) {
    this.post({ type: "scopeData", channels, samples });
  }

  post(message) {
    this.iframe.contentWindow?.postMessage(
      { __ondaRunHost: true, message },
      this.targetOrigin,
    );
  }

  async handleMessage(message) {
    if (!message) return;
    try {
      switch (message.type) {
        case "webviewReady":
          this.ready = true;
          this.postState();
          break;
        case "start":
          if (this.state.sourceDirty || this.buffersReady()) await this.handlers.start?.();
          else this.setWaitingForBuffers();
          break;
        case "stop":
          await this.handlers.stop?.();
          break;
        case "reset":
          await this.handlers.reset?.();
          break;
        case "setParam":
          this.state.params = this.state.params.map((param) =>
            param.name === message.name ? { ...param, value: message.value } : param
          );
          await this.handlers.setParam?.(message.name, message.value);
          break;
        case "triggerEvent":
          await this.handlers.triggerEvent?.(message.name, message.values ?? []);
          break;
        case "bindBufferFile":
          await this.handlers.bindBufferFile?.(message.name, message.file);
          break;
        case "clearBuffer":
          await this.handlers.clearBuffer?.(message.name);
          break;
        case "refreshDevices":
        case "setInputDevice":
        case "setOutputDevice":
        case "chooseBufferFile":
          break;
      }
    } catch (error) {
      this.handlers.error?.(error);
    }
  }

  dispose() {
    window.removeEventListener("message", this.onWindowMessage);
    this.themeObserver.disconnect();
  }
}

export class BrowserScopeSource {
  constructor(runView) {
    this.runView = runView;
    this.timer = 0;
    this.analysers = [];
    this.samples = [];
  }

  start(context, node, channels) {
    this.stop();
    if (!channels) return;
    this.splitter = context.createChannelSplitter(channels);
    this.silentGain = context.createGain();
    this.silentGain.gain.value = 0;
    node.connect(this.splitter);
    for (let channel = 0; channel < channels; channel += 1) {
      const analyser = context.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0;
      this.splitter.connect(analyser, channel, 0);
      analyser.connect(this.silentGain);
      this.analysers.push(analyser);
      this.samples.push(new Float32Array(analyser.fftSize));
    }
    this.silentGain.connect(context.destination);
    this.timer = setInterval(() => this.capture(), 50);
    this.capture();
  }

  capture() {
    const channels = this.analysers.length;
    if (!channels) return;
    for (let channel = 0; channel < channels; channel += 1) {
      this.analysers[channel].getFloatTimeDomainData(this.samples[channel]);
    }
    const frames = this.samples[0].length;
    const interleaved = new Array(frames * channels);
    for (let frame = 0; frame < frames; frame += 1) {
      for (let channel = 0; channel < channels; channel += 1) {
        interleaved[frame * channels + channel] = this.samples[channel][frame];
      }
    }
    this.runView.postScope(channels, interleaved);
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
}

export function mergeParams(params, existing) {
  return params
    .filter((param) => param.array_len === 1)
    .map((param, index) => {
      const previous = existing.find((item) => item.name === param.name);
      const next = {
        index,
        name: param.name,
        type: param.scalar,
        default: decodeScalarRepr(param.scalar, param.default_reprs?.[0]),
        rangeMin: decodeScalarRepr(param.scalar, param.range_min_repr),
        rangeMax: decodeScalarRepr(param.scalar, param.range_max_repr),
        scalar: true,
      };
      return {
        ...next,
        value: previous && paramShapeMatches(next, previous)
          ? previous.value
          : initialParamValue(next),
      };
    });
}

function mergeBuffers(buffers, existing, bufferFiles) {
  return buffers.map((buffer, index) => ({
    index,
    name: buffer.name,
    type: buffer.type_repr,
    channelsKind: buffer.channels,
    channelsStatic: buffer.static_channels ?? null,
    loadedPath: bufferFiles.get(buffer.name)?.name
      ?? existing.find((item) => item.name === buffer.name)?.loadedPath
      ?? null,
  }));
}

function mergeEvents(events, existing) {
  return events.map((event, index) => {
    const previous = existing.find((item) => item.name === event.name);
    return {
      index,
      name: event.name,
      args: (event.params ?? []).map((param, argIndex) => {
        const prior = previous?.args.find((arg) => arg.name === param.name);
        const next = {
          index: argIndex,
          name: param.name,
          type: param.scalar,
          default: param.array_len === 1
            ? decodeScalarRepr(param.scalar, param.default_reprs?.[0])
            : null,
        };
        return { ...next, value: prior?.value ?? initialEventArgValue(next) };
      }),
    };
  });
}

function paramShapeMatches(left, right) {
  return left.type === right.type
    && left.default === right.default
    && left.rangeMin === right.rangeMin
    && left.rangeMax === right.rangeMax;
}

function initialParamValue(param) {
  if (param.type === "bool") return Boolean(param.default);
  if (param.default !== null && param.default !== undefined) return param.default;
  if (param.rangeMin !== null && param.rangeMin !== undefined) return param.rangeMin;
  return 0;
}

function initialEventArgValue(arg) {
  if (arg.type === "bool") return Boolean(arg.default);
  return Number.isFinite(Number(arg.default)) ? Number(arg.default) : 0;
}

function decodeScalarRepr(type, value) {
  if (value === null || value === undefined) return null;
  if (type === "bool") return value === "true";
  if (type !== "f32" && type !== "f64") return Number(value);
  if (!value.startsWith("0x")) return Number(value);
  const width = type === "f32" ? 32 : 64;
  const digits = value.startsWith("0x") ? value.slice(2) : "";
  if (digits.length !== width / 4) return Number.NaN;
  const bytes = new ArrayBuffer(width / 8);
  const view = new DataView(bytes);
  if (width === 32) {
    view.setUint32(0, Number.parseInt(digits, 16), false);
    return view.getFloat32(0, false);
  }
  view.setBigUint64(0, BigInt(value), false);
  return view.getFloat64(0, false);
}
