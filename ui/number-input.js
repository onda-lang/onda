// Shared single-number interaction for embedded run views and browser settings.
(() => {
  const styledDocuments = new WeakSet();

  function formatNumberForEditing(value) {
    if (value === 0) return "0";
    const text = Math.abs(value) < 0.0001 || Math.abs(value) >= 1e9
      ? value.toExponential(5) : value.toFixed(5);
    const [mantissa, exponent] = text.split("e");
    const trimmed = mantissa.replace(/\.?0+$/, "");
    return exponent === undefined ? trimmed : `${trimmed}e${exponent.replace(/^\+/, "")}`;
  }

  function bindDraggableNumber(input, {
    speed,
    dragRange = null,
    updateValue,
    resetValue = () => Number(input.defaultValue),
    dragTarget = input,
    readValue = () => input.valueAsNumber,
  }) {
    const document = input.ownerDocument;
    if (!styledDocuments.has(document)) {
      const style = document.createElement("style");
      style.textContent = `
        input[data-draggable]:read-only {
          appearance: textfield;
          cursor: ew-resize;
          touch-action: none;
          user-select: none;
        }
        input[data-draggable]::-webkit-inner-spin-button,
        input[data-draggable]::-webkit-outer-spin-button {
          appearance: none;
          margin: 0;
        }
      `;
      document.head.appendChild(style);
      styledDocuments.add(document);
    }

    let gesture = null;
    let editValue = null;
    input.dataset.draggable = "";
    input.readOnly = true;
    input.title = [input.title,
      "Drag left/right to adjust; Shift for fine adjustment; click to type; double-click to reset."]
      .filter(Boolean).join("\n");
    dragTarget.title = input.title;
    input.addEventListener("focus", () => {
      // Cancellation restores the stored value, not the rounded editing text.
      editValue = String(readValue());
      input.readOnly = false;
    });
    input.addEventListener("blur", () => {
      editValue = null;
      input.readOnly = true;
    });
    input.addEventListener("keydown", event => {
      if (event.isComposing || input.readOnly) return;
      if (event.key === "Escape" && editValue !== null) {
        event.preventDefault();
        // Mark cancellation before blur so owners ignore native change events.
        input.readOnly = true;
        input.value = editValue;
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.blur();
      } else if (event.key === "Enter") {
        event.preventDefault();
        input.blur();
      }
    });

    const constrain = value => {
      const convert = typeof value === "bigint" ? BigInt : Number;
      if (input.min !== "" && value < convert(input.min)) return convert(input.min);
      if (input.max !== "" && value > convert(input.max)) return convert(input.max);
      return value;
    };
    const move = event => {
      if (!gesture || event.pointerId !== gesture.pointerId) return;
      if (!gesture.dragging) {
        if (Math.abs(event.clientX - gesture.startX) < 3) return;
        // Take ownership when dragging starts, after any intervening host updates.
        const value = readValue();
        if (typeof value !== "bigint" && !Number.isFinite(value)) return;
        gesture.value = value;
        if (dragRange !== null) {
          gesture.rangePosition = dragRange.domain.plainToNormalized(value) * dragRange.pixels;
        }
        gesture.dragging = true;
      }
      const movement = (event.clientX - gesture.previousX) * (event.shiftKey ? 0.1 : 1);
      const delta = movement * speed;
      gesture.previousX = event.clientX;
      if (typeof gesture.value === "bigint") {
        // Accumulate sub-integer motion without converting the value to a float.
        const amount = gesture.remainder + delta;
        const units = Math.round(amount);
        const next = gesture.value + BigInt(units);
        gesture.value = constrain(next);
        const remainder = amount - units;
        // Fractional overshoot must not delay reversal at a bound.
        const outward = (remainder < 0 && input.min !== "" && gesture.value === BigInt(input.min))
          || (remainder > 0 && input.max !== "" && gesture.value === BigInt(input.max));
        gesture.remainder = gesture.value === next && !outward ? remainder : 0;
      } else if (dragRange !== null) {
        // Accumulate pixels before scaling so subnormal ranges retain small movements.
        gesture.rangePosition = Math.max(0, Math.min(dragRange.pixels, gesture.rangePosition + movement));
        gesture.value = dragRange.domain.normalizedToPlain(gesture.rangePosition / dragRange.pixels);
      } else {
        gesture.value = constrain(gesture.value + delta);
      }
      if (typeof gesture.value === "bigint" || Number.isFinite(gesture.value)) {
        updateValue(gesture.value, false);
      }
    };
    const finish = (event, commit = true) => {
      if (!gesture || (event && event.pointerId !== gesture.pointerId)) return;
      if (event?.type === "pointerup") move(event);
      const { pointerId, dragging, value } = gesture;
      gesture = null;
      dragTarget.removeEventListener("pointermove", move);
      dragTarget.removeEventListener("pointerup", finish);
      dragTarget.removeEventListener("pointercancel", finish);
      dragTarget.removeEventListener("lostpointercapture", finish);
      if (dragTarget.hasPointerCapture(pointerId)) dragTarget.releasePointerCapture(pointerId);
      if (dragging && commit) {
        updateValue(value, true);
      } else if (event?.type === "pointerup") {
        input.readOnly = false;
        input.focus();
        input.select();
      }
    };
    dragTarget.addEventListener("pointerdown", event => {
      if (input.disabled || !input.readOnly || event.button !== 0 || gesture) return;
      const value = readValue();
      if (typeof value !== "bigint" && !Number.isFinite(value)) return;
      event.preventDefault();
      gesture = {
        pointerId: event.pointerId,
        startX: event.clientX,
        previousX: event.clientX,
        value,
        remainder: 0,
        rangePosition: 0,
        dragging: false,
      };
      dragTarget.setPointerCapture(event.pointerId);
      dragTarget.addEventListener("pointermove", move);
      dragTarget.addEventListener("pointerup", finish);
      dragTarget.addEventListener("pointercancel", finish);
      dragTarget.addEventListener("lostpointercapture", finish);
    });
    dragTarget.addEventListener("dblclick", event => {
      if (input.disabled || event.button !== 0) return;
      event.preventDefault();
      finish();
      input.readOnly = true;
      updateValue(resetValue(), true, "reset");
      input.blur();
    });
    return {
      isDragging: () => gesture !== null,
      finish: () => finish(),
      cancel: () => finish(undefined, false),
    };
  }

  globalThis.__ONDA_NUMBER_INPUT__ = { bindDraggableNumber, formatNumberForEditing };
})();
