import { once } from "node:events";

// Firefox's built-in BiDi endpoint supplies trusted input without a driver.
export async function openBrowserSession(t, endpoint) {
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
  t.signal.addEventListener("abort", () => failPending(new Error("Browser test timed out")), { once: true });
  const call = (method, params) => {
    const request = Promise.withResolvers();
    const id = ++nextId;
    pending.set(id, request);
    socket.send(JSON.stringify({ id, method, params }));
    return request.promise;
  };
  await call("session.new", { capabilities: {} });
  return call;
}
