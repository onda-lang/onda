// Own the browser preference independently of each processor's lifetime.
export class BrowserParamSmoothing {
  constructor(maxSampleRate, getProcessor, onError, storage) {
    this.maxSampleRate = maxSampleRate;
    this.getProcessor = getProcessor;
    this.onError = onError;
    this.storageKey = "onda.browser-ide.param-smoothing-ms.v1";
    this.milliseconds = 30;
    try {
      this.storage = storage ?? globalThis.localStorage;
      const saved = this.storage?.getItem(this.storageKey);
      const value = Number(saved);
      if (saved != null && saved.trim() && this.isValid(value)) this.milliseconds = value;
    } catch {}
  }

  isValid(milliseconds) {
    return Number.isFinite(milliseconds) && milliseconds >= 0
      && Number.isSafeInteger(Math.ceil(milliseconds / 1000 * this.maxSampleRate));
  }

  set(milliseconds) {
    if (!this.isValid(milliseconds)) {
      throw new Error("Smoothing duration must be finite, non-negative, and fit the host sample counter");
    }
    this.milliseconds = milliseconds;
    try { this.storage?.setItem(this.storageKey, String(milliseconds)); } catch {}
    const processor = this.getProcessor();
    processor?.setParamSmoothingSeconds(milliseconds / 1000).catch(error => {
      if (processor === this.getProcessor()) this.onError(error);
    });
  }
}
