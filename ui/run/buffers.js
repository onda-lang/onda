// Buffer cards and file drops shared by native webviews and browser hosts.
(() => {
  const filePrompt = "Choose or drop a file to bind the buffer";
  const isBufferFile = name => /\.(wav|ondabuffer)$/i.test(name);

  class BufferView {
    constructor(node, bridge, canEdit) {
      this.node = node;
      this.bridge = bridge;
      this.canEdit = canEdit;
      this.target = null;
      this.browserPosition = null;
      this.nativePaths = [];
      this.nativePosition = null;
      this.renderedKey = null;

      const document = node.ownerDocument;
      for (const type of ["dragenter", "dragover"]) {
        document.addEventListener(type, event => {
          const transfer = event.dataTransfer;
          if (!transfer?.types.includes("Files")) return;
          event.preventDefault();
          const files = [...transfer.items].filter(item => item.kind === "file");
          const accepted = files.length === 1
            && (!transfer.files.length || isBufferFile(transfer.files[0].name));
          const target = accepted ? this.cardFor(event.target) : null;
          this.browserPosition = target ? { x: event.clientX, y: event.clientY } : null;
          this.setTarget(target);
          transfer.dropEffect = target ? "copy" : "none";
        });
      }
      document.addEventListener("dragleave", event => {
        if (!this.target?.contains(event.relatedTarget)) this.clearDrag();
      });
      document.addEventListener("dragend", () => this.clearDrag());
      document.addEventListener("drop", event => {
        const transfer = event.dataTransfer;
        if (!transfer?.types.includes("Files")) return;
        // Prevent the webview from navigating to files dropped outside a card.
        event.preventDefault();
        this.clearDrag();
        const card = this.cardFor(event.target);
        const file = transfer.files.length === 1 ? transfer.files[0] : null;
        if (!card || !file || !isBufferFile(file.name)) return;
        if (bridge.mode === "browser") {
          bridge.postMessage({ type: "bindBufferFile", name: card.dataset.bufferName, file });
        }
      });
    }

    cardFor(element) {
      const card = element?.closest?.(".buffer-card");
      return this.canEdit() && card && this.node.contains(card) ? card : null;
    }

    setTarget(card) {
      if (this.target === card) return;
      this.target?.classList.remove("file-hover");
      this.target = card;
      card?.classList.add("file-hover");
    }

    clearDrag() {
      this.browserPosition = null;
      this.nativePaths = [];
      this.nativePosition = null;
      this.setTarget(null);
    }

    cardAt(position) {
      return position
        ? this.cardFor(this.node.ownerDocument.elementFromPoint(position.x, position.y))
        : null;
    }

    nativeTarget() {
      if (!this.nativePosition || this.nativePaths.length !== 1
        || !isBufferFile(this.nativePaths[0])) return null;
      return this.cardAt(this.nativePosition);
    }

    nativeDrag(message) {
      if (message.phase === "leave") {
        this.clearDrag();
        return;
      }
      if (message.paths) this.nativePaths = message.paths;
      this.nativePosition = message.position;
      const card = this.nativeTarget();
      this.setTarget(card);
      if (message.phase === "drop") {
        const filePath = this.nativePaths[0];
        this.clearDrag();
        if (card) {
          this.bridge.postMessage({
            type: "bindBufferFile", name: card.dataset.bufferName, filePath,
          });
        }
      }
    }

    drawWaveforms() {
      drawVisibleBufferWaveforms(this.node);
    }

    render(buffers) {
      const canEditRun = this.canEdit();
      const key = JSON.stringify([canEditRun, buffers]);
      if (key !== this.renderedKey) {
        this.renderCards(buffers, canEditRun);
        this.renderedKey = key;
      }

      if (canEditRun) {
        this.setTarget(this.nativePosition ? this.nativeTarget() : this.cardAt(this.browserPosition));
      } else {
        this.clearDrag();
      }
    }

    renderCards(buffers, canEditRun) {
      const fragment = this.node.ownerDocument.createDocumentFragment();
      for (const buffer of buffers) {
        const card = document.createElement("div");
        card.className = "buffer-card";
        card.dataset.bufferName = buffer.name;
        card.title = filePrompt;

        const head = document.createElement("div");
        head.className = "buffer-identity";
        head.innerHTML = '<div class="param-name"></div><div class="param-type"></div>';
        head.querySelector(".param-name").textContent = buffer.name;
        head.querySelector(".param-type").textContent = buffer.type;
        card.appendChild(head);

        const detail = document.createElement("div");
        detail.className = "buffer-detail";
        const loaded = document.createElement("div");
        loaded.className = "buffer-file";
        const isBound = Number.isInteger(Number(buffer.loadedFrames))
          && Number(buffer.loadedFrames) > 0;
        loaded.textContent = buffer.loadedPath
          ? bufferFileName(buffer.loadedPath)
          : isBound
            ? "Embedded project asset"
            : filePrompt;
        loaded.title = buffer.loadedPath || loaded.textContent;
        detail.appendChild(loaded);
        const metadata = loadedBufferSummary(buffer);
        if (metadata) {
          const summary = document.createElement("div");
          summary.className = "buffer-metadata";
          summary.textContent = metadata;
          detail.appendChild(summary);
        }
        card.appendChild(detail);

        const actions = document.createElement("div");
        actions.className = "buffer-actions";
        const choose = document.createElement("button");
        choose.className = "secondary compact-action";
        choose.textContent = isBound ? "Replace" : "Bind";
        choose.title = filePrompt;
        choose.disabled = !canEditRun;
        choose.addEventListener("click", () => {
          if (typeof this.bridge.chooseBufferFile === "function") {
            this.bridge.chooseBufferFile(buffer.name);
          } else {
            this.bridge.postMessage({ type: "chooseBufferFile", name: buffer.name });
          }
        });
        const clear = document.createElement("button");
        clear.className = "secondary compact-action";
        clear.textContent = "Clear";
        clear.hidden = !isBound;
        clear.disabled = !canEditRun || !isBound;
        clear.addEventListener("click", () => {
          this.bridge.postMessage({ type: "clearBuffer", name: buffer.name });
        });
        actions.append(choose, clear);
        card.appendChild(actions);

        fragment.appendChild(card);
        const waveform = bufferWaveform(buffer);
        if (waveform) {
          const preview = document.createElement("div");
          preview.className = "buffer-waveform";
          const canvas = document.createElement("canvas");
          canvas.setAttribute("role", "img");
          canvas.setAttribute("aria-label", bufferWaveformSummary(waveform));
          canvas.title = bufferWaveformSummary(waveform);
          canvas.__ondaBufferWaveform = waveform;
          preview.appendChild(canvas);
          card.appendChild(preview);
        }
      }

      // Canvas measurement forces layout. Commit every card before drawing so
      // the scroller never observes an incomplete, shorter buffer section.
      this.node.replaceChildren(fragment);
      this.drawWaveforms();
    }
  }

  function bufferFileName(path) {
    const parts = String(path).split(/[\\/]/);
    return parts[parts.length - 1] || String(path);
  }

  function loadedBufferSummary(buffer) {
    const frames = Number(buffer.loadedFrames);
    const channels = Number(buffer.loadedChannels);
    const sampleRate = Number(buffer.loadedSampleRate);
    if (
      !Number.isSafeInteger(frames) || frames < 0
      || !Number.isSafeInteger(channels) || channels <= 0
      || !Number.isFinite(sampleRate) || sampleRate <= 0
    ) {
      return "";
    }
    return frames.toLocaleString() + " frames \u00b7 "
      + channels + " ch \u00b7 " + formatBufferSampleRate(sampleRate);
  }

  function bufferWaveform(buffer) {
    const waveform = buffer && buffer.waveform;
    const minValue = Number(waveform && waveform.minValue);
    const maxValue = Number(waveform && waveform.maxValue);
    const minimums = waveform && waveform.minimums;
    const maximums = waveform && waveform.maximums;
    if (
      !Number.isFinite(minValue)
      || !Number.isFinite(maxValue)
      || !Array.isArray(minimums)
      || !Array.isArray(maximums)
      || minimums.length === 0
      || minimums.length !== maximums.length
    ) return null;
    return { minValue, maxValue, minimums, maximums };
  }

  function bufferWaveformScale(waveform) {
    return Math.max(1, Math.abs(waveform.minValue), Math.abs(waveform.maxValue));
  }

  function formatWaveformValue(value) {
    const magnitude = Math.abs(value);
    let rendered;
    if (Object.is(value, -0) || value === 0) {
      rendered = "0";
    } else if (magnitude >= 10_000 || magnitude < 0.001) {
      rendered = value.toExponential(2);
    } else {
      rendered = value.toFixed(3).replace(/\.?0+$/, "");
    }
    return rendered.replaceAll("-", "\u2212");
  }

  function bufferWaveformSummary(waveform) {
    return ("Minimum: " + waveform.minValue + "\nMaximum: " + waveform.maxValue)
      .replaceAll("-", "\u2212");
  }

  function drawBufferWaveform(canvas, waveform) {
    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    const cssWidth = rect.width || canvas.parentElement?.clientWidth || 600;
    const cssHeight = rect.height || 62;
    const width = Math.max(1, Math.round(cssWidth * dpr));
    const height = Math.max(1, Math.round(cssHeight * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const context = canvas.getContext("2d");
    const computed = getComputedStyle(document.documentElement);
    const gridColor = computed.getPropertyValue("--scope-grid").trim();
    const strokeColor = computed.getPropertyValue("--scope-1").trim();
    const textColor = computed.getPropertyValue("--muted").trim();
    const monoFont = computed.getPropertyValue("--mono-font").trim();
    context.clearRect(0, 0, width, height);

    const scale = bufferWaveformScale(waveform);
    const labels = [scale, 0, -scale].map(formatWaveformValue);
    context.font = `${9.5 * dpr}px ${monoFont}`;
    const labelWidth = Math.max(...labels.map(label => context.measureText(label).width));
    const left = 5 * dpr + labelWidth + 5 * dpr;
    const right = width - 5 * dpr;
    const top = 5 * dpr;
    const bottom = height - 5 * dpr;
    const center = (top + bottom) / 2;
    const amplitude = (bottom - top) / 2;
    const snapToPixelCenter = coordinate => Math.floor(coordinate) + 0.5;
    const zeroY = snapToPixelCenter(center);
    const valueY = value => zeroY - value / scale * amplitude;
    const guideY = value => snapToPixelCenter(valueY(value));

    context.strokeStyle = gridColor;
    context.lineWidth = 1;
    context.setLineDash([]);
    context.save();
    context.globalAlpha = 0.4;
    context.lineWidth = 1;
    context.beginPath();
    context.moveTo(left, zeroY);
    context.lineTo(right, zeroY);
    context.stroke();
    context.restore();
    if (scale > 1 && Math.abs(guideY(1) - zeroY) >= 2) {
      context.setLineDash([2 * dpr, 2 * dpr]);
      for (const value of [-1, 1]) {
        const y = guideY(value);
        context.beginPath();
        context.moveTo(left, y);
        context.lineTo(right, y);
        context.stroke();
      }
      context.setLineDash([]);
    }

    context.strokeStyle = strokeColor;
    context.lineWidth = Math.max(1, 1.25 * dpr);
    const plotWidth = Math.max(1, right - left);
    const columns = waveform.minimums.length;
    const bucketX = index => snapToPixelCenter(
      left + plotWidth * (index + 0.5) / columns
    );
    const sampleY = value => snapToPixelCenter(
      valueY(Math.max(-scale, Math.min(scale, value)))
    );
    context.beginPath();
    for (let index = 0; index < columns; index++) {
      const minimum = Number(waveform.minimums[index]);
      const maximum = Number(waveform.maximums[index]);
      if (!Number.isFinite(minimum) || !Number.isFinite(maximum)) continue;
      const x = bucketX(index);
      const maximumY = maximum > 0 ? sampleY(maximum) : zeroY;
      const minimumY = minimum < 0 ? sampleY(minimum) : zeroY;
      context.moveTo(x, maximumY);
      context.lineTo(x, minimumY);
    }
    context.stroke();

    context.fillStyle = strokeColor;
    context.beginPath();
    const markerRadius = 0.75 * dpr;
    for (let index = 0; index < columns; index++) {
      const minimum = Number(waveform.minimums[index]);
      const maximum = Number(waveform.maximums[index]);
      if (!Number.isFinite(minimum) || !Number.isFinite(maximum)) continue;
      const x = bucketX(index);
      const maximumY = sampleY(maximum);
      context.moveTo(x + markerRadius, maximumY);
      context.arc(x, maximumY, markerRadius, 0, 2 * Math.PI);
      if (minimum !== maximum) {
        const minimumY = sampleY(minimum);
        context.moveTo(x + markerRadius, minimumY);
        context.arc(x, minimumY, markerRadius, 0, 2 * Math.PI);
      }
    }
    context.fill();

    context.fillStyle = textColor;
    context.textAlign = "right";
    for (const [label, y, baseline] of [
      [labels[0], top, "top"],
      [labels[1], center, "middle"],
      [labels[2], bottom, "bottom"],
    ]) {
      context.textBaseline = baseline;
      context.fillText(label, left - 5 * dpr, y);
    }
  }

  function drawVisibleBufferWaveforms(node) {
    for (const canvas of node.querySelectorAll(".buffer-waveform canvas")) {
      if (canvas.__ondaBufferWaveform) {
        drawBufferWaveform(canvas, canvas.__ondaBufferWaveform);
      }
    }
  }

  function formatBufferSampleRate(sampleRate) {
    if (sampleRate < 1_000) return Math.round(sampleRate) + " Hz";
    const khz = sampleRate / 1_000;
    return (Number.isInteger(khz) ? khz.toFixed(0) : khz.toFixed(1)) + " kHz";
  }



  globalThis.__ONDA_RUN_BUFFERS__ = { BufferView, formatBufferSampleRate };
})();
