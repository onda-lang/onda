// Native disclosure keeps Project actions and typography in the tab strip.
export function bindProjectMenu(menu) {
  const trigger = menu.querySelector("summary");
  const close = (restoreFocus = false) => {
    if (!menu.open) return;
    menu.open = false;
    if (restoreFocus) trigger.focus();
  };
  menu.addEventListener("click", event => {
    if (event.target.closest("button")) close(true);
  });
  menu.ownerDocument.addEventListener("keydown", event => {
    if (!menu.open || event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    close(true);
  });
  menu.addEventListener("focusout", event => {
    if (event.relatedTarget && !menu.contains(event.relatedTarget)) close();
  });
  menu.ownerDocument.addEventListener("pointerdown", event => {
    if (!menu.contains(event.target)) close();
  });
  menu.ownerDocument.defaultView.addEventListener("blur", () => close());
}
