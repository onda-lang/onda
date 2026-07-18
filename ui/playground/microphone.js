export class BrowserMicrophoneInput {
  constructor(mediaDevices = globalThis.navigator?.mediaDevices) {
    this.mediaDevices = mediaDevices;
    this.streamPromise = null;
    this.stream = null;
    this.source = null;
    this.permissionRequestCount = 0;
  }

  get permissionRequested() {
    return this.permissionRequestCount > 0;
  }

  async connect(context, destination, inputChannels) {
    const channels = Number(inputChannels);
    if (!Number.isSafeInteger(channels) || channels < 0) {
      throw new Error("microphone input channel count must be a non-negative integer");
    }
    if (channels === 0) {
      this.disconnect();
      return false;
    }
    if (typeof context?.createMediaStreamSource !== "function") {
      throw new Error("this browser cannot connect microphone audio to Web Audio");
    }

    const stream = await this.acquire(channels);
    this.disconnectSource();
    for (const track of stream.getAudioTracks()) track.enabled = true;
    this.source = context.createMediaStreamSource(stream);
    this.source.connect(destination);
    return true;
  }

  disconnect() {
    this.disconnectSource();
    for (const track of this.stream?.getAudioTracks?.() ?? []) track.enabled = false;
  }

  close() {
    this.disconnectSource();
    for (const track of this.stream?.getTracks?.() ?? []) track.stop();
    this.stream = null;
    this.streamPromise = null;
  }

  disconnectSource() {
    try {
      this.source?.disconnect();
    } catch {
      // A source can already be disconnected while an AudioContext is closing.
    }
    this.source = null;
  }

  acquire(inputChannels) {
    if (this.streamPromise) return this.streamPromise;
    if (typeof this.mediaDevices?.getUserMedia !== "function") {
      throw new Error(
        "microphone input requires browser media-device support and a secure HTTPS context",
      );
    }

    this.permissionRequestCount += 1;
    this.streamPromise = Promise.resolve().then(() => this.mediaDevices.getUserMedia({
      audio: {
        autoGainControl: false,
        channelCount: { ideal: inputChannels },
        echoCancellation: false,
        noiseSuppression: false,
      },
      video: false,
    })).then((stream) => {
      if (!stream?.getAudioTracks?.().length) {
        throw new Error("the selected media stream does not contain a microphone audio track");
      }
      this.stream = stream;
      return stream;
    }).catch((error) => {
      const detail = String(error?.message ?? error);
      if (error?.name === "NotAllowedError" || error?.name === "SecurityError") {
        throw new Error(`microphone permission was not granted: ${detail}`, { cause: error });
      }
      if (error?.name === "NotFoundError") {
        throw new Error(`no microphone input is available: ${detail}`, { cause: error });
      }
      throw error;
    });
    return this.streamPromise;
  }
}
