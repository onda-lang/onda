import assert from "node:assert/strict";
import test from "node:test";

import { BrowserMicrophoneInput } from "./microphone.js";

function microphoneFixture() {
  const track = { enabled: true, stopped: false, stop() { this.stopped = true; } };
  const stream = {
    getAudioTracks: () => [track],
    getTracks: () => [track],
  };
  const requests = [];
  const mediaDevices = {
    async getUserMedia(constraints) {
      requests.push(constraints);
      return stream;
    },
  };
  const sources = [];
  const context = {
    createMediaStreamSource(value) {
      const source = {
        stream: value,
        connected: null,
        disconnected: false,
        connect(destination) { this.connected = destination; },
        disconnect() { this.disconnected = true; },
      };
      sources.push(source);
      return source;
    },
  };
  return { context, mediaDevices, requests, sources, stream, track };
}

test("does not request microphone permission without top-level audio inputs", async () => {
  const fixture = microphoneFixture();
  const microphone = new BrowserMicrophoneInput(fixture.mediaDevices);

  assert.equal(await microphone.connect(fixture.context, {}, 0), false);
  assert.equal(fixture.requests.length, 0);
  assert.equal(microphone.permissionRequested, false);
});

test("requests one microphone stream and reuses it across processor restarts", async () => {
  const fixture = microphoneFixture();
  const microphone = new BrowserMicrophoneInput(fixture.mediaDevices);
  const firstDestination = {};
  const secondDestination = {};

  assert.equal(await microphone.connect(fixture.context, firstDestination, 2), true);
  assert.equal(fixture.requests.length, 1);
  assert.deepEqual(fixture.requests[0], {
    audio: {
      autoGainControl: false,
      channelCount: { ideal: 2 },
      echoCancellation: false,
      noiseSuppression: false,
    },
    video: false,
  });
  assert.equal(fixture.sources[0].connected, firstDestination);

  microphone.disconnect();
  assert.equal(fixture.track.enabled, false);
  assert.equal(await microphone.connect(fixture.context, secondDestination, 2), true);
  assert.equal(fixture.requests.length, 1);
  assert.equal(fixture.track.enabled, true);
  assert.equal(fixture.sources[1].stream, fixture.stream);
  assert.equal(fixture.sources[1].connected, secondDestination);

  microphone.close();
  assert.equal(fixture.track.stopped, true);
});
