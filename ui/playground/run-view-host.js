// Shared browser adapter for the transport-neutral Onda run view.
import { flattenedAudioChannelCount } from "@onda-lang/webaudio";
import { canonicalF32Number } from "@onda-lang/processor-abi";
import { partitionHostEvents } from "./midi.js";
import { wireJson } from "../gpu/protocol.js";
import { bindAudioActivation } from "./audio-activation.js";

export class BrowserRunViewHost {
  constructor(iframe, handlers = {}) {
    this.iframe = iframe;
    this.targetOrigin = new URL(iframe.src, window.location.href).origin;
    this.handlers = handlers;
    this.ready = false;
    this.viewFailed = false;
    this.pendingViewReset = true;
    this.viewResetId = 0;
    this.viewState = this.readViewState();
    this.logFrame = 0;
    this.sourceFiles = [];
    this.setLoading(true);
    this.state = {
      running: false,
      connected: false,
      schemaReady: false,
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
      supportsMidiInputSelection: true,
      supportsViewState: true,
      logText: "",
      logEntries: [],
      logRevealed: false,
      printOverflowCount: 0,
      printTransportDropCount: 0,
      supportsRunSettings: typeof handlers.setRunSettings === "function",
      canSetSampleRate: typeof handlers.setRunSettings === "function",
      canSetBlockSize: typeof handlers.setRunSettings === "function",
      supportsParamSmoothing: typeof handlers.setParamSmoothing === "function",
      paramSmoothingMs: null,
      paramSmoothingDefaultMs: null,
      supportsScope: true,
      sampleRateHz: 48_000,
      blockSamples: 512,
      fastMath: false,
      themeMode: document.documentElement.dataset.theme || "auto",
    };
    this.onWindowMessage = (event) => {
      if (
        event.source !== this.iframe.contentWindow ||
        event.origin !== this.targetOrigin ||
        !event.data?.__ondaRunUi
      )
        return;
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
    this.onFrameLoad = () => {
      this.ready = false;
      this.viewFailed = false;
      this.requestViewReset();
      this.unbindAudioActivation?.();
      this.unbindAudioActivation = bindAudioActivation(
        this.iframe.contentDocument,
        this.handlers,
      );
      this.post({ type: "requestRunViewReady" });
    };
    this.iframe.addEventListener("load", this.onFrameLoad);
  }

  setPath(path) {
    this.setState({ path });
  }

  setLoading(loading) {
    loading &&= !this.viewFailed;
    this.iframe.style.opacity = loading ? "0" : "";
    this.handlers.loadingChanged?.(loading);
  }

  requestViewReset() {
    this.pendingViewReset = true;
    this.viewResetId += 1;
    this.setLoading(true);
  }

  setState(update, { resetViewState = false } = {}) {
    if (
      resetViewState ||
      (update.path !== undefined && update.path !== this.state.path)
    ) {
      if (resetViewState) {
        this.viewState = null;
        try {
          window.localStorage?.removeItem("onda.browser-ide.run-view.v1");
        } catch {}
      }
      this.requestViewReset();
    }
    Object.assign(this.state, update);
    this.postState({}, update);
  }

  setCompiling(path, options = null) {
    this.setState({
      path,
      running: false,
      connected: false,
      schemaReady: false,
      status: "Compiling...",
      error: "",
      sampleRateHz: options?.sampleRate ?? this.state.sampleRateHz,
      blockSamples: options?.blockSize ?? this.state.blockSamples,
      fastMath: options?.codegen?.fastMath ?? this.state.fastMath,
    });
  }

  setArtifact(artifact, bufferFiles) {
    const metadata = artifact.metadata.metadata;
    this.sourceFiles = metadata.source_files ?? [];
    const hostEvents = partitionHostEvents(metadata.events ?? []);
    this.state.params = mergeParams(metadata.params ?? [], this.state.params);
    const firstSchema = this.state.events.length === 0;
    this.state.events = mergeEvents(hostEvents.visible, this.state.events);
    if (firstSchema) this.restoreEventValues(this.viewState);
    this.state.midi = hostEvents.midi;
    this.handlers.midiEventsChanged?.(hostEvents.declaredMidi);
    this.state.buffers = mergeBuffers(
      metadata.buffers ?? [],
      this.state.buffers,
      bufferFiles,
    );
    this.state.outputChannels = flattenedAudioChannelCount(metadata.outputs);
    this.state.schemaReady = true;
    const sampleRateHz = Number(artifact.metadata.compile?.sample_rate);
    const blockSamples = Number(artifact.metadata.compile?.block_size);
    this.state.fastMath = artifact.metadata.compile?.fast_math === true;
    if (Number.isFinite(sampleRateHz) && sampleRateHz > 0) {
      this.state.sampleRateHz = sampleRateHz;
    }
    if (Number.isInteger(blockSamples) && blockSamples > 0) {
      this.state.blockSamples = blockSamples;
    }
    this.state.error = "";
    this.state.sourceDirty = false;
    this.postState({ resetEventArguments: true });
  }

  clearArtifact(path = this.state.path) {
    if (this.logFrame) cancelAnimationFrame(this.logFrame);
    this.logFrame = 0;
    this.sourceFiles = [];
    this.setState(
      {
        path,
        running: false,
        connected: false,
        schemaReady: true,
        status: "Stopped",
        error: "",
        sourceDirty: false,
        outputChannels: 0,
        fastMath: false,
        buffers: [],
        events: [],
        midi: { available: false, noteOn: false, noteOff: false },
        params: [],
        logText: "",
        logEntries: [],
        logRevealed: false,
        printOverflowCount: 0,
        printTransportDropCount: 0,
      },
      { resetViewState: true },
    );
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
      schemaReady: true,
      status,
    });
    this.postScope(0, []);
  }

  setError(error) {
    this.setState({
      running: false,
      connected: false,
      schemaReady: true,
      status: "Stopped",
      error: formatError(error),
    });
  }

  showError(error) {
    this.setState({ error: formatError(error) });
  }

  resetParamValues() {
    this.state.params = this.state.params.map((param) => ({
      ...param,
      value: initialParamValue(param),
    }));
    this.postParameterValues(this.state.params);
  }

  resetEventArguments() {
    this.state.events = this.state.events.map((event) => ({
      ...event,
      args: event.args.map((arg) => ({
        ...arg,
        value: initialEventArgValue(arg),
      })),
    }));
    this.postState({ resetEventArguments: true });
  }

  updateBufferFile(name, file, metadata = null) {
    this.updateBufferFiles([{ name, file, metadata }]);
  }

  updateBufferFiles(updates) {
    const byName = new Map(updates.map((update) => [update.name, update]));
    this.state.buffers = this.state.buffers.map((buffer) => {
      const update = byName.get(buffer.name);
      if (!update) return buffer;
      const { file, metadata } = update;
      return {
        ...buffer,
        loadedPath: file?.name ?? null,
        loadedFrames: file ? (metadata?.frames ?? null) : null,
        loadedChannels: file ? (metadata?.channels ?? null) : null,
        loadedSampleRate: file ? (metadata?.sampleRate ?? null) : null,
        waveform: file ? (metadata?.waveform ?? null) : null,
      };
    });
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

  setMidiActivity(activeNotes) {
    this.post({ type: "midiActivity", activeNotes });
  }

  readViewState() {
    try {
      return JSON.parse(
        window.localStorage?.getItem("onda.browser-ide.run-view.v1") ?? "null",
      );
    } catch {
      return null;
    }
  }

  saveViewState(view) {
    if (!view || view.path !== this.state.path) return;
    const json = wireJson(view);
    if (json.length > 256 * 1024) return;
    this.viewState = view;
    this.restoreEventValues(view);
    try {
      window.localStorage?.setItem(
        "onda.browser-ide.run-view.v1",
        json,
      );
    } catch {}
  }

  restoreEventValues(view) {
    if (
      !view ||
      view.path !== this.state.path ||
      !Array.isArray(view.eventValues)
    )
      return;
    for (const event of this.state.events) {
      const saved = view.eventValues?.find((item) => item.name === event.name);
      if (
        Array.isArray(saved?.values) &&
        saved.values.length === event.args.length &&
        Array.isArray(saved.args) &&
        saved.args.length === event.args.length &&
        event.args.every((arg, i) =>
          arg.name === saved.args[i]?.name &&
          eventDefaultsMatch(arg.shape, saved.args[i].shape) &&
          Object.hasOwn(arg, "default") &&
          Object.hasOwn(saved.args[i], "default") &&
          eventDefaultsMatch(
            JSON.parse(wireJson(arg.default)),
            JSON.parse(wireJson(saved.args[i].default)),
          ),
        )
      ) {
        for (let i = 0; i < event.args.length; i++)
          event.args[i].value = structuredClone(saved.values[i]);
      }
    }
  }

  appendPrint({ text = "", entries = [], overflowCount = 0, transportDropCount = 0 }) {
    const previous = this.state.logText;
    const lines = text.split("\n");
    if (lines.at(-1) === "") lines.pop();
    // A partial line keeps its original context when the next batch completes it.
    const continued = previous.length > 0 && !previous.endsWith("\n");
    for (let i = Number(continued); i < lines.length; i++) {
      const entry = entries[i];
      const source = entry?.source;
      // Only context crosses the UI bridge; decoded values can contain BigInts.
      this.state.logEntries.push(
        entry ? {
          source: source && {
            ...source,
            file: Number.isInteger(source.file)
              ? this.sourceFiles[source.file]?.path : source.file,
          },
          lexicalOwner: entry.lexicalOwner,
        } : null,
      );
    }
    const combined = previous + text;
    let removed = Math.max(0, combined.length - 65536);
    // Drop the whole code point when the budget cuts a surrogate pair in half.
    if (removed > 0 && combined.codePointAt(removed - 1) > 0xffff) removed++;
    const removedLines = combined.slice(0, removed).split("\n").length - 1;
    this.state.logText = combined.slice(removed);
    this.state.logEntries.splice(0, removedLines);
    this.state.logRevealed = true;
    this.state.printOverflowCount += overflowCount;
    this.state.printTransportDropCount += transportDropCount;
    if (!this.logFrame)
      this.logFrame = requestAnimationFrame(() => {
        this.logFrame = 0;
        this.post({
          type: "state",
          state: {
            logText: this.state.logText,
            logEntries: this.state.logEntries,
            logRevealed: true,
            printOverflowCount: this.state.printOverflowCount,
            printTransportDropCount: this.state.printTransportDropCount,
          },
        });
      });
  }

  postState(extra = {}, update = this.state) {
    if (
      !this.ready ||
      !this.state.path ||
      (this.pendingViewReset && this.state.schemaReady === false)
    ) return;
    const saved =
      this.viewState?.path === this.state.path
        ? this.viewState
        : { reset: true };
    this.post({
      type: "state",
      state: {
        ...(this.pendingViewReset ? this.state : update),
        ...extra,
        ...(this.pendingViewReset
          ? { viewState: { ...saved, readyId: this.viewResetId } }
          : {}),
      },
    });
    this.pendingViewReset = false;
  }

  postParameterValues(params) {
    if (!this.ready) return;
    if (this.pendingViewReset) {
      this.postState();
      return;
    }
    this.post({
      type: "parameterValues",
      state: {
        params: params.map(({ name, value }) => ({ name, value })),
      },
    });
  }

  postScope(channels, samples) {
    this.post({ type: "scopeData", channels, samples });
  }

  postScopePacket(packet) {
    this.post({ type: "scopeData", packet });
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
        case "uiReady":
          this.viewFailed = false;
          this.ready = true;
          this.postState();
          break;
        case "runViewReady":
          if (!this.pendingViewReset && message.readyId === this.viewResetId)
            this.setLoading(false);
          break;
        case "runViewError":
          this.viewFailed = true;
          this.setLoading(false);
          break;
        case "scopeConfig":
          this.handlers.scopeConfig?.(message);
          break;
        case "viewState":
          this.saveViewState(message.state);
          break;
        case "clearLog":
          if (this.logFrame) cancelAnimationFrame(this.logFrame);
          this.logFrame = 0;
          this.setState({
            logText: "",
            logEntries: [],
            logRevealed: false,
            printOverflowCount: 0,
            printTransportDropCount: 0,
          });
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
        case "setRunSettings":
          if (this.state.supportsRunSettings) {
            await this.handlers.setRunSettings({
              sampleRateHz: message.sampleRateHz,
              blockSamples: message.blockSamples,
            });
          }
          break;
        case "setParamSmoothing": {
          const milliseconds = message.milliseconds;
          if (!this.state.supportsParamSmoothing) break;
          try {
            if (!Number.isFinite(milliseconds) || milliseconds < 0) {
              throw new Error(
                "Smoothing duration must be a finite, non-negative number",
              );
            }
            await this.handlers.setParamSmoothing(milliseconds);
            this.setState({ paramSmoothingMs: milliseconds, error: "" });
          } catch (error) {
            this.showError(error);
          }
          break;
        }
        case "setParam":
          this.state.params = this.state.params.map((param) =>
            param.name === message.name
              ? { ...param, value: message.value }
              : param,
          );
          this.postParameterValues([
            { name: message.name, value: message.value },
          ]);
          await this.handlers.setParam?.(message.name, message.value);
          break;
        case "triggerEvent": {
          const event = this.state.events.find(
            (item) => item.name === message.name,
          );
          const values = message.values ?? [];
          await this.handlers.triggerEvent?.(
            message.name,
            event
              ? event.args.map((arg, i) => eventValue(arg.shape, values[i]))
              : values,
          );
          break;
        }
        case "midiNote": {
          const name = message.pressed
            ? this.state.midi.noteOn
              ? "note_on"
              : null
            : this.state.midi.noteOff
              ? "note_off"
              : null;
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
    if (this.logFrame) cancelAnimationFrame(this.logFrame);
    window.removeEventListener("message", this.onWindowMessage);
    this.iframe.removeEventListener("load", this.onFrameLoad);
    this.unbindAudioActivation?.();
    this.themeObserver.disconnect();
  }
}

export { BrowserScopeSource } from "./browser-scope.js";

function formatError(error) {
  const message = String(error?.message ?? error);
  const diagnostic = error?.diagnostics?.[0];
  if (!Number.isInteger(diagnostic?.line) || diagnostic.line <= 0) return message;
  const file = diagnostic.file ? `${diagnostic.file}:` : "line ";
  const column = Number.isInteger(diagnostic.column) && diagnostic.column > 0
    ? `:${diagnostic.column}` : "";
  return `${message} (${file}${diagnostic.line}${column})`;
}

export function mergeParams(params, existing) {
  return params.flatMap((root, index) =>
    Array.from({ length: root.array_len }, (_, element) => {
      const isArray = root.type_repr !== root.scalar;
      const param = {
        ...root,
        name: isArray ? `${root.name}[${element}]` : root.name,
      };

      const previous = existing.find((item) => item.name === param.name);
      const control = param.param_control;
      const next = {
        index,
        name: param.name,
        type: param.scalar,
        default: decodeScalarRepr(param.scalar, param.default_reprs?.[element]),
        rangeMin: decodeScalarRepr(param.scalar, param.range_min_repr),
        rangeMax: decodeScalarRepr(param.scalar, param.range_max_repr),
        scale: control?.scale ?? null,
        curve: control?.curve ?? null,
        unit: control?.unit ?? null,
        step: decodeScalarRepr(param.scalar, control?.step_repr),
        stepCount: control?.step_count ?? null,
        scalar: true,
        array: isArray
          ? { name: root.name, length: root.array_len, index: element }
          : null,
      };
      return {
        ...next,
        value:
          previous && paramShapeMatches(next, previous)
            ? previous.value
            : initialParamValue(next),
      };
    }),
  );
}

function mergeBuffers(buffers, existing, bufferFiles) {
  return buffers.map((buffer, index) => {
    const previous = existing.find((item) => item.name === buffer.name);
    const loadedPath =
      bufferFiles.get(buffer.name)?.name ?? previous?.loadedPath ?? null;
    const preservesLoadedFile =
      loadedPath !== null && previous?.loadedPath === loadedPath;
    return {
      index,
      name: buffer.name,
      type: buffer.type_repr,
      channelsKind: buffer.channels,
      channelsStatic: buffer.static_channels ?? null,
      loadedPath,
      loadedFrames: preservesLoadedFile
        ? (previous.loadedFrames ?? null)
        : null,
      loadedChannels: preservesLoadedFile
        ? (previous.loadedChannels ?? null)
        : null,
      loadedSampleRate: preservesLoadedFile
        ? (previous.loadedSampleRate ?? null)
        : null,
      waveform: preservesLoadedFile ? (previous.waveform ?? null) : null,
    };
  });
}

export function mergeEvents(events, existing) {
  return events.map((event, index) => {
    const previous = existing.find((item) => item.name === event.name);
    return {
      index,
      name: event.name,
      args: event.schema.params.map((param, argIndex) => {
        const prior = previous?.args.find((arg) => arg.name === param.name);
        const ty = param.ty;
        const scalar =
          ty.kind === "scalar" ? ty.encoding : (ty.element?.encoding ?? null);
        const next = {
          index: argIndex,
          name: param.name,
          type: payloadTypeName(ty),
          scalar,
          arrayLength:
            ty.kind === "slice" ? null : ty.kind === "array" ? ty.len : 1,
          isSlice: ty.kind === "slice",
          default: payloadDefault(ty, param.default),
          shape: structuredClone(ty),
        };
        return {
          ...next,
          value:
            prior && eventArgShapeMatches(next, prior)
              ? prior.value
              : initialEventArgValue(next),
        };
      }),
    };
  });
}

function paramShapeMatches(left, right) {
  return (
    left.type === right.type &&
    left.array?.name === right.array?.name &&
    left.array?.length === right.array?.length &&
    left.default === right.default &&
    left.rangeMin === right.rangeMin &&
    left.rangeMax === right.rangeMax &&
    left.scale === right.scale &&
    left.curve === right.curve &&
    left.unit === right.unit &&
    left.step === right.step &&
    left.stepCount === right.stepCount
  );
}

function eventArgShapeMatches(left, right) {
  return (
    left.type === right.type &&
    eventDefaultsMatch(left.shape, right.shape) &&
    left.scalar === right.scalar &&
    left.arrayLength === right.arrayLength &&
    left.isSlice === right.isSlice &&
    eventDefaultsMatch(left.default, right.default)
  );
}

function eventDefaultsMatch(left, right) {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left)) {
    return (
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) => eventDefaultsMatch(value, right[index]))
    );
  }
  if (
    Array.isArray(right) ||
    left === null ||
    right === null ||
    typeof left !== "object" ||
    typeof right !== "object"
  ) {
    return false;
  }
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key) =>
        Object.hasOwn(right, key) && eventDefaultsMatch(left[key], right[key]),
    )
  );
}

function payloadTypeName(ty) {
  switch (ty.kind) {
    case "scalar":
      return ty.encoding;
    case "struct":
      return ty.name;
    case "tuple":
      return `(${ty.elements.map(payloadTypeName).join(", ")})`;
    case "array":
      return `${payloadTypeName(ty.element)}[${ty.len}]`;
    case "slice":
      return `${payloadTypeName(ty.element)}[]`;
  }
}

function payloadDefault(ty, value) {
  switch (ty.kind) {
    case "scalar":
      return (
        decodeEventScalarRepr(ty.encoding, value) ??
        (ty.encoding === "bool" ? false : ty.encoding === "i64" ? "0" : 0)
      );
    case "struct":
      return Object.fromEntries(
        ty.fields.map((field, index) => [
          field.name,
          payloadDefault(field.ty, value?.[index] ?? field.default),
        ]),
      );
    case "tuple":
      return ty.elements.map((element, index) =>
        payloadDefault(element, value?.[index]),
      );
    case "array":
      return Array.from({ length: ty.len }, (_, index) =>
        payloadDefault(ty.element, value?.[index]),
      );
    case "slice":
      return [];
  }
}

function initialParamValue(param) {
  if (param.type === "bool") return Boolean(param.default);
  if (param.default !== null && param.default !== undefined)
    return param.default;
  if (param.rangeMin !== null && param.rangeMin !== undefined)
    return param.rangeMin;
  return 0;
}

function initialEventArgValue(arg) {
  return structuredClone(
    arg.default ?? (arg.type === "bool" ? false : arg.type === "i64" ? "0" : 0),
  );
}

function decodeEventScalarRepr(type, value) {
  if (type === "i64")
    return value === null || value === undefined ? null : String(value);
  return decodeScalarRepr(type, value);
}

function decodeScalarRepr(type, value) {
  if (value === null || value === undefined) return null;
  if (type === "bool") return value === "true";
  if (type !== "f32" && type !== "f64") return Number(value);
  if (!value.startsWith("0x")) {
    const decoded = Number(value);
    return type === "f32" ? canonicalF32Number(decoded) : decoded;
  }
  const width = type === "f32" ? 32 : 64;
  const digits = value.startsWith("0x") ? value.slice(2) : "";
  if (digits.length !== width / 4) return Number.NaN;
  const bytes = new ArrayBuffer(width / 8);
  const view = new DataView(bytes);
  if (width === 32) {
    view.setUint32(0, Number.parseInt(digits, 16), false);
    return canonicalF32Number(view.getFloat32(0, false));
  }
  view.setBigUint64(0, BigInt(value), false);
  return view.getFloat64(0, false);
}

// The shader UI uses JSON strings for non-finite floats; the Web Audio API uses
// JavaScript numbers. Wide integers remain decimal strings at both boundaries.
function eventValue(shape, value) {
  switch (shape?.kind) {
    case "scalar":
      return ["f32", "f64"].includes(shape.encoding) &&
        typeof value === "string"
        ? Number(value)
        : value;
    case "struct":
      return Object.fromEntries(
        shape.fields.map((f) => [f.name, eventValue(f.ty, value?.[f.name])]),
      );
    case "tuple":
      return shape.elements.map((s, i) => eventValue(s, value?.[i]));
    case "array":
    case "slice":
      return Array.isArray(value)
        ? value.map((v) => eventValue(shape.element, v))
        : value;
    default:
      return value;
  }
}
