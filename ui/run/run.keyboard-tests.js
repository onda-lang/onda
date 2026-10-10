import assert from "node:assert/strict";
import { once } from "node:events";

// Trusted keyboard actions exercise native input/change behavior that dispatched
// DOM events cannot reproduce. Firefox exposes BiDi without a separate driver.
export async function testNumberKeyboardEditing(t, endpoint, pageUrl) {
  const socket = new WebSocket(`${endpoint}/session`);
  t.after(() => socket.close());
  await once(socket, "open", { signal: t.signal });
  let nextId = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const reply = JSON.parse(data);
    const request = pending.get(reply.id);
    if (!request) return;
    pending.delete(reply.id);
    if (reply.type === "error") request.reject(new Error(JSON.stringify(reply)));
    else request.resolve(reply.result);
  });
  const failPending = error => {
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  };
  socket.addEventListener("close", () => failPending(new Error("Browser connection closed")));
  t.signal.addEventListener("abort", () => failPending(new Error("Keyboard test timed out")), { once: true });
  const call = (method, params) => {
    const request = Promise.withResolvers();
    const id = ++nextId;
    pending.set(id, request);
    socket.send(JSON.stringify({ id, method, params }));
    return request.promise;
  };
  await call("session.new", { capabilities: {} });
  let context;
  const evaluate = async expression => {
    const reply = await call("script.evaluate", {
      expression, target: { context }, awaitPromise: true,
    });
    assert.equal(reply.type, "success", JSON.stringify(reply));
    return reply.result.value;
  };
  const keys = values => call("input.performActions", {
    context,
    actions: [{ type: "key", id: "keyboard", actions: values.flatMap(value => [
      { type: "keyDown", value }, { type: "keyUp", value },
    ]) }],
  });
  const initial = 1023.095538565032;
  const initialize = async layout => {
    // Each case starts with a fresh document and focus, including after Tab has
    // left the page for browser chrome.
    if (context) await call("browsingContext.close", { context });
    ({ context } = await call("browsingContext.create", { type: "tab" }));
    await call("browsingContext.navigate", { context, url: pageUrl, wait: "complete" });
    await evaluate(`
      window._onHostMessage({ type: "state", state: {
        running: true, connected: true, path: "keyboard.onda", events: [],
        params: [{ name: "precision", type: "f64",
          value: ${initial}, default: ${initial} }],
      }});
      document.querySelector('[data-param-layout="${layout}"]').click();
      window.__testMessages.length = 0;
      window.number = document.querySelector('#params input');
      number.focus();
      number.select();
    `);
    assert.equal(await evaluate("number.readOnly"), false, "field must enter editing before keyboard actions");
    assert.equal(await evaluate("number.value"), "1023.09554");
  };
  const messages = async () => JSON.parse(await evaluate(
    `JSON.stringify(__testMessages.filter(message => message.type === "setParam"))`,
  ));
  const enter = "\uE007";
  const escape = "\uE00C";
  const tab = "\uE004";
  const up = "\uE013";
  for (const layout of ["knobs", "sliders"]) {
    for (const end of [enter, escape, tab]) {
      await initialize(layout);
      await keys([end]);
      assert.deepEqual(await messages(), [], `${layout}: untouched drafts retain precision`);
    }
    await initialize(layout);
    await keys(["9", "9", escape]);
    assert.deepEqual(await messages(), [], `${layout}: Escape must not commit the shortened draft`);

    await initialize(layout);
    await keys(["9", "9"]);
    await evaluate(`window._onHostMessage({ type: "state", state: { params: [{
      name: number.getAttribute("aria-label"), type: "f64", value: 2048.987654321,
      default: ${initial},
    }] }});`);
    await keys([escape]);
    assert.deepEqual(await messages(), [], `${layout}: Escape reconciles host automation without a commit`);
    assert.equal(await evaluate("number.value"), "2048.988");

    for (const [inputKeys, expected, description] of [
      [["9", "9", enter], 99, "Enter commits typed input once"],
      [[up, enter], 1023.096, "Enter commits an arrow edit matching the idle display"],
      [[up, tab], 1023.096, "blur commits an arrow edit matching the idle display"],
      [[..."1023.096", enter], 1023.096, "typing the idle display still changes the canonical value"],
    ]) {
      await initialize(layout);
      await keys(inputKeys);
      const sent = await messages();
      assert.equal(sent.length, 1, `${layout}: ${description}`);
      assert.equal(sent[0].value, expected, `${layout}: ${description}`);
      assert.equal(sent[0].commit, true);
      assert.equal(await evaluate("number.readOnly"), true);
    }
    t.diagnostic(`${layout}: trusted keyboard edits preserve cancellation precision and commit on Enter or blur`);
  }
  const eventValues = [initial, 0.0000123456789];
  const initializeEvent = async index => {
    await initialize("knobs");
    await evaluate(`
      window._onHostMessage({ type: "state", state: {
        params: [], events: [{ name: "precision", args: [
          { name: "scalar", type: "f64", default: ${eventValues[0]} },
          { name: "elements", type: "f64[1]", arrayLength: 1, default: [${eventValues[1]}] },
        ] }],
      }});
      window.number = document.querySelectorAll('#events input')[${index}];
      number.focus();
      number.select();
    `);
    assert.equal(await evaluate("number.value"), ["1023.09554", "1.23457e-5"][index]);
  };
  const triggeredValues = async () => JSON.parse(await evaluate(`
    document.querySelector('.event-trigger').click();
    JSON.stringify(__testMessages.findLast(message => message.type === 'triggerEvent').values);
  `));
  for (const index of [0, 1]) {
    for (const inputKeys of [[enter], [escape], [tab], ["9", "9", escape]]) {
      await initializeEvent(index);
      await keys(inputKeys);
      assert.deepEqual(await triggeredValues(), [eventValues[0], [eventValues[1]]],
        `event field ${index}: ${JSON.stringify(inputKeys)} retains the exact event values`);
    }
    await initializeEvent(index);
    await keys([..."0.123456789012345", enter]);
    const values = await triggeredValues();
    assert.equal(index === 0 ? values[0] : values[1][0], 0.123456789012345,
      "event editors accept more precise typed values than their initial draft");
  }
  t.diagnostic("event scalars and array elements shorten focused drafts without losing stored or typed precision");
  await call("session.end", {});
}
