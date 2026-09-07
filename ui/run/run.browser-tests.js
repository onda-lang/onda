// Executed against the real run view DOM by run.test.js.
const results = [];
function check(condition, message) {
  if (!condition) throw new Error(message);
  results.push(message);
}
function send(state) {
  window._onHostMessage({ type: "state", state: structuredClone(state) });
}
function edit(input, value) {
  input.focus();
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
function refresh() {
  send({ events, params, logText: "background update", logRevealed: true });
}
const events = [{
  name: "shape",
  args: [
    { name: "level", type: "f32", arrayLength: 1, default: 0.25 },
    { name: "id", type: "i64", arrayLength: 1, default: "0" },
    { name: "points", type: "f32[]", isSlice: true, arrayLength: 0, default: [] },
  ],
}];
const params = [
  { name: "gain", type: "f32", value: 0.25, default: 0.25,
    rangeMin: 0, rangeMax: 1, scale: "linear" },
  { name: "offset", type: "f32", value: 2, default: 2 },
];
try {
  send({ running: true, connected: true, path: "test.onda", status: "Active",
    supportsTransport: false, events, params });
  const scalar = document.querySelector("#events input");
  edit(scalar, "0.75");
  for (let i = 0; i < 20; ++i) refresh();
  check(document.querySelector("#events input") === scalar,
    "event controls survive metadata and log refreshes");
  check(document.activeElement === scalar && scalar.value === "0.75",
    "event focus and edited value survive refreshes");
  document.querySelector(".event-trigger").click();
  check(window.__testMessages.at(-1).values[0] === 0.75,
    "retained event handlers dispatch the edited arguments");

  const integer = document.querySelector('#events input[type="text"]');
  edit(integer, "-");
  integer.setSelectionRange(1, 1);
  refresh();
  check(document.activeElement === integer && integer.value === "-"
    && integer.selectionStart === 1,
    "incomplete i64 drafts and caret survive refreshes");
  send({ events, resetEventArguments: true });
  check(document.querySelector('#events input[type="text"]').value === "0",
    "explicit reset discards a draft even when its canonical value is unchanged");
  check(document.querySelector("#events input").value === "0.75",
    "reset flag alone does not invent replacement values");
  send({ events: events.map(event => ({ ...event,
    args: event.args.map(arg => ({ ...arg, value: arg.default })),
  })), resetEventArguments: true });
  check(document.querySelector("#events input").value === "0.25",
    "new program or explicit argument reset restores defaults");

  document.querySelector('[aria-label="Add element"]').click();
  let arrayInput = document.querySelector(".event-array-element input");
  edit(arrayInput, "0.625");
  refresh();
  check(document.querySelector(".event-array-element input") === arrayInput
    && arrayInput.value === "0.625", "slice edits survive refreshes");
  document.querySelector(".event-trigger").click();
  check(window.__testMessages.at(-1).values[2][0] === 0.625,
    "slice handlers retain current values");
  document.querySelector('[aria-label="Remove last element"]').click();
  check(!document.querySelector(".event-array-element input"),
    "slice shape changes rebuild the necessary controls");

  for (let index = 0; index < params.length; ++index) {
    const input = document.querySelectorAll('#params input[type="number"]')[index];
    edit(input, "0.83");
    refresh();
    check(input.value === "0.83" && document.activeElement === input,
      `numeric draft ${index} survives a host refresh`);
    input.dispatchEvent(new Event("change", { bubbles: true }));
    input.blur();
    const command = window.__testMessages.at(-1);
    check(command.type === "setParam" && command.name === params[index].name
      && Math.abs(command.value - 0.83) < 0.001 && command.commit === true,
      `numeric draft ${index} commits its edited value`);
    input.focus();
    params[index].value = 0.6;
    refresh();
    check(Math.abs(Number(input.value) - 0.83) < 0.001,
      `focused field ${index} is not overwritten by automation`);
    input.blur();
    check(Math.abs(Number(input.value) - 0.6) < 0.001,
      `field ${index} reconciles host state when focus leaves without an edit`);
  }

  const arrayParams = ["f32", "f64", "i32", "i64", "bool"].flatMap(type =>
    [0, 1].map(index => ({ name: `${type}Values[${index}]`, type,
      value: type === "bool" ? false : index, default: type === "bool" ? false : index,
      rangeMin: type === "bool" ? null : 0, rangeMax: type === "bool" ? null : 10,
      scale: type === "bool" ? null : "linear",
      step: type.startsWith("i") ? 1 : null, stepCount: type.startsWith("i") ? 10 : null,
      array: { name: `${type}Values`, length: 2, index },
    })));
  send({ params: arrayParams });
  check(document.querySelectorAll(".param-array-heading").length === 5,
    "primitive arrays have one group heading each");
  check(document.querySelectorAll("#params .param").length === 10,
    "array controls follow their declared lengths");
  const boolInput = document.querySelector('#params input[type="checkbox"]');
  boolInput.click();
  check(window.__testMessages.at(-1).name === "boolValues[0]"
    && window.__testMessages.at(-1).value === true,
    "boolean array controls send an indexed address");
  send({ params: arrayParams });
  check(document.querySelector('#params input[type="checkbox"]') === boolInput,
    "array controls survive metadata refreshes");

  send({ connected: false });
  check(document.querySelector(".event-trigger").disabled
    && document.querySelector("#events input").disabled,
    "disconnect disables retained event controls");
  send({ connected: true, events: [] });
  check(!document.querySelector(".event-trigger"), "unload removes event controls");
  await fetch("/result", { method: "POST", body: JSON.stringify({ results }) });
} catch (error) {
  await fetch("/result", { method: "POST",
    body: JSON.stringify({ results, error: error.stack }) });
}
