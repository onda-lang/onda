// Keep native select values intact while shortening their visible labels on phones.
export function bindCompileSettingLabels(root) {
  const compact = root.defaultView.matchMedia("(width < 400px)");
  const selects = root.querySelectorAll("[data-sample-rate], [data-block-size]");
  const sync = () => {
    for (const select of selects) {
      for (const option of select.options) {
        option.label = compact.matches ? option.value : option.textContent;
        option.setAttribute("aria-label", option.textContent);
      }
    }
  };
  compact.addEventListener("change", sync);
  sync();
}
