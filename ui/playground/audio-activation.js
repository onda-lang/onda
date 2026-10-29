// Run inside each document's trusted input event, including the embedded view.
export function bindAudioActivation(ownerDocument, { resume, stop }) {
  if (!ownerDocument || !resume) return () => {};
  const explicit = Boolean(ownerDocument.querySelector?.("#onda-ui"));
  const eventTypes = explicit
    ? ["ondaRunGesture"]
    : ["pointerdown", "pointerup", "keydown"];
  const onInteraction = (received) => {
    const event = explicit ? received.detail.event : received;
    const explicitStop = explicit && received.detail.stop;
    if (!event.isTrusted) return;
    if (event.type === "pointerdown" && event.pointerType !== "mouse") return;
    if (event.type === "pointerup" && event.pointerType === "mouse") return;
    if (
      event.type === "keydown" &&
      (event.isComposing ||
        event.repeat ||
        ["Escape", "Control", "Meta", "Alt", "Shift"].includes(event.key))
    )
      return;

    const stopShortcut =
      event.type === "keydown" &&
      !event.altKey &&
      (event.ctrlKey || event.metaKey) &&
      (event.key === "." || event.code === "Period");
    const stopButton = event.target?.closest?.("#stop");
    const activatesStop =
      stopButton &&
      !stopButton.disabled &&
      (event.type !== "keydown" || event.key === "Enter" || event.key === " ");
    if (explicitStop || stopShortcut || activatesStop) {
      void stop?.();
      return;
    }
    resume();
  };
  for (const type of eventTypes) {
    ownerDocument.addEventListener(type, onInteraction, { capture: true });
  }
  return () => {
    for (const type of eventTypes) {
      ownerDocument.removeEventListener(type, onInteraction, { capture: true });
    }
  };
}
