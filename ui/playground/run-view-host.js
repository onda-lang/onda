// Shared browser adapter for the transport-neutral Onda run view.
import { flattenedAudioChannelCount } from "@onda-lang/webaudio";
import { partitionHostEvents } from "./midi.js";

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
      midi: { available: false, noteOn: false, noteOff: false },
      params: [],
      inputDevices: [],
      outputDevices: [],
      midiInputDevices: [],
      currentInputDevice: null,
      currentOutputDevice: null,
      currentMidiInputDevice: null,
      supportsSourceSelection: false,
      supportsTransport: true,
      supportsDeviceSelection: false,
      supportsRunSettings: false,
      supportsScope: true,
      sampleRateHz: 48_000,
      blockFrames: 512,
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

  setCompiling(path, options = null) {
    this.setState({
      path,
      running: false,
      connected: false,
      status: "Compiling",
      error: "",
      sampleRateHz: options?.sampleRate ?? this.state.sampleRateHz,
      blockFrames: options?.blockSize ?? this.state.blockFrames,
    });
  }

  setArtifact(artifact, bufferFiles) {
    const metadata = artifact.metadata.metadata;
    const hostEvents = partitionHostEvents(metadata.events ?? []);
    this.state.params = mergeParams(metadata.params ?? [], this.state.params);
    this.state.events = mergeEvents(hostEvents.visible, this.state.events);
    this.state.midi = hostEvents.midi;
    this.handlers.midiEventsChanged?.(hostEvents.declaredMidi);
    this.state.buffers = mergeBuffers(metadata.buffers ?? [], this.state.buffers, bufferFiles);
    this.state.outputChannels = flattenedAudioChannelCount(metadata.outputs);
    const sampleRateHz = Number(artifact.metadata.compile?.sample_rate);
    const blockFrames = Number(artifact.metadata.compile?.block_size);
    if (Number.isFinite(sampleRateHz) && sampleRateHz > 0) {
      this.state.sampleRateHz = sampleRateHz;
    }
    if (Number.isInteger(blockFrames) && blockFrames > 0) {
      this.state.blockFrames = blockFrames;
    }
    this.state.error = "";
    this.state.sourceDirty = false;
    this.postState();
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
      midi: { available: false, noteOn: false, noteOff: false },
      params: [],
    });
    this.handlers.midiEventsChanged?.(new Set());
    this.postScope(0, []);
  }

  markSourceDirty(path = this.state.path) {
    this.setState({ path, sourceDirty: true });
  }

  setRunning(sampleRate, status) {
    this.setState({
      running: true,
      connected: true,
      status: status ?? "Running",
      sampleRateHz: sampleRate,
      error: "",
    });
  }

  setStopped(status = "Stopped") {
    this.setState({
      running: false,
      connected: false,
      status,
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

  resetParamValues() {
    this.state.params = this.state.params.map((param) => ({
      ...param,
      value: initialParamValue(param),
    }));
    this.postState();
  }

  resetEventArguments() {
    this.state.events = this.state.events.map((event) => ({
      ...event,
      args: event.args.map((arg) => ({ ...arg, value: initialEventArgValue(arg) })),
    }));
    this.postState();
  }

  updateBufferFile(name, file, metadata = null) {
    this.state.buffers = this.state.buffers.map((buffer) =>
      buffer.name === name
        ? {
            ...buffer,
            loadedPath: file?.name ?? null,
            loadedFrames: file ? metadata?.frames ?? null : null,
            loadedChannels: file ? metadata?.channels ?? null : null,
            loadedSampleRate: file ? metadata?.sampleRate ?? null : null,
          }
        : buffer
    );
    this.state.error = "";
    this.postState();
  }

  setMidiInputs(devices, current) {
    this.setState({
      midiInputDevices: devices,
      currentMidiInputDevice: current,
    });
  }

  sendComputerKey(code, pressed) {
    this.post({ type: "computerKey", code, pressed });
  }

  releaseVirtualMidiNotes() {
    this.post({ type: "releaseVirtualMidiNotes" });
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
          await this.handlers.start?.();
          break;
        case "stop":
          await this.handlers.stop?.();
          break;
        case "resetParams":
          this.resetParamValues();
          await this.handlers.resetParams?.();
          break;
        case "resetEventArguments":
          this.resetEventArguments();
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
        case "midiNote": {
          const name = message.pressed
            ? (this.state.midi.noteOn ? "note_on" : null)
            : (this.state.midi.noteOff ? "note_off" : null);
          if (name) {
            const key = Math.max(0, Math.min(127, Number(message.key) || 0));
            const velocity = message.pressed
              ? Math.max(0, Math.min(1, Number(message.velocity) || 0))
              : 0;
            await this.handlers.triggerEvent?.(name, [-1, 0, key, velocity]);
          }
          break;
        }
        case "bindBufferFile":
          await this.handlers.bindBufferFile?.(message.name, message.file);
          break;
        case "clearBuffer":
          await this.handlers.clearBuffer?.(message.name);
          break;
        case "refreshDevices":
          await this.handlers.refreshMidiInputs?.();
          break;
        case "setMidiInputDevice":
          await this.handlers.setMidiInputDevice?.(message.name ?? null);
          break;
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
      const control = param.param_control;
      const next = {
        index,
        name: param.name,
        type: param.scalar,
        default: decodeScalarRepr(param.scalar, param.default_reprs?.[0]),
        rangeMin: decodeScalarRepr(param.scalar, param.range_min_repr),
        rangeMax: decodeScalarRepr(param.scalar, param.range_max_repr),
        scale: control?.scale ?? null,
        curve: control?.curve ?? null,
        unit: control?.unit ?? null,
        step: decodeScalarRepr(param.scalar, control?.step_repr),
        stepCount: control?.step_count ?? null,
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
  return buffers.map((buffer, index) => {
    const previous = existing.find((item) => item.name === buffer.name);
    const loadedPath = bufferFiles.get(buffer.name)?.name ?? previous?.loadedPath ?? null;
    const preservesLoadedFile = loadedPath !== null && previous?.loadedPath === loadedPath;
    return {
      index,
      name: buffer.name,
      type: buffer.type_repr,
      channelsKind: buffer.channels,
      channelsStatic: buffer.static_channels ?? null,
      loadedPath,
      loadedFrames: preservesLoadedFile ? previous.loadedFrames ?? null : null,
      loadedChannels: preservesLoadedFile ? previous.loadedChannels ?? null : null,
      loadedSampleRate: preservesLoadedFile ? previous.loadedSampleRate ?? null : null,
    };
  });
}

export function mergeEvents(events, existing) {
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
          type: param.type_repr,
          scalar: param.scalar,
          arrayLength: param.is_slice ? null : param.array_len,
          isSlice: param.is_slice === true,
          default: param.is_slice
            ? []
            : param.array_len === 1
              ? decodeEventScalarRepr(param.scalar, param.default_reprs?.[0])
              : Array.from(
                { length: param.array_len },
                (_, valueIndex) =>
                  decodeEventScalarRepr(param.scalar, param.default_reprs?.[valueIndex])
                  ?? (param.scalar === "bool" ? false : param.scalar === "i64" ? "0" : 0),
              ),
        };
        return {
          ...next,
          value: prior && eventArgShapeMatches(next, prior)
            ? prior.value
            : initialEventArgValue(next),
        };
      }),
    };
  });
}

function paramShapeMatches(left, right) {
  return left.type === right.type
    && left.default === right.default
    && left.rangeMin === right.rangeMin
    && left.rangeMax === right.rangeMax
    && left.scale === right.scale
    && left.curve === right.curve
    && left.unit === right.unit
    && left.step === right.step
    && left.stepCount === right.stepCount;
}

function eventArgShapeMatches(left, right) {
  return left.type === right.type
    && left.scalar === right.scalar
    && left.arrayLength === right.arrayLength
    && left.isSlice === right.isSlice
    && eventDefaultsMatch(left.default, right.default);
}

function eventDefaultsMatch(left, right) {
  if (!Array.isArray(left) || !Array.isArray(right)) return left === right;
  return left.length === right.length
    && left.every((value, index) => value === right[index]);
}

function initialParamValue(param) {
  if (param.type === "bool") return Boolean(param.default);
  if (param.default !== null && param.default !== undefined) return param.default;
  if (param.rangeMin !== null && param.rangeMin !== undefined) return param.rangeMin;
  return 0;
}

function initialEventArgValue(arg) {
  if (
    arg.isSlice
    || (typeof arg.arrayLength === "number" && arg.arrayLength !== 1)
    || (typeof arg.type === "string" && /\[[0-9]*\]$/.test(arg.type))
  ) {
    return Array.isArray(arg.default) ? [...arg.default] : [];
  }
  if (arg.type === "bool") return Boolean(arg.default);
  if (arg.type === "i64") return typeof arg.default === "string" ? arg.default : "0";
  return Number.isFinite(Number(arg.default)) ? Number(arg.default) : 0;
}

function decodeEventScalarRepr(type, value) {
  if (type === "i64") return value === null || value === undefined ? null : String(value);
  return decodeScalarRepr(type, value);
}

function decodeScalarRepr(type, value) {
  if (value === null || value === undefined) return null;
  if (type === "bool") return value === "true";
  if (type !== "f32" && type !== "f64") return Number(value);
  if (!value.startsWith("0x")) {
    const decoded = Number(value);
    return type === "f32" ? Math.fround(decoded) : decoded;
  }
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
