import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL(".", import.meta.url)));
const port = Number(process.env.PORT || 8787);

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

let lastResult = {
  status: "pending",
};

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (url.pathname === "/__result") {
      if (req.method === "GET") {
        res.writeHead(200, { "Content-Type": "application/json; charset=utf-8" });
        res.end(JSON.stringify(lastResult));
        return;
      }

      if (req.method === "POST") {
        let body = "";
        for await (const chunk of req) {
          body += chunk;
        }
        const parsed = JSON.parse(body || "{}");
        lastResult = {
          status: "complete",
          ...parsed,
        };
        res.writeHead(204);
        res.end();
        return;
      }

      res.writeHead(405, { "Content-Type": "text/plain; charset=utf-8" });
      res.end("method not allowed");
      return;
    }

    const requestPath = url.pathname;
    const relativePath = requestPath === "/" ? "/index.html" : requestPath;
    const filePath = resolve(join(root, normalize(relativePath)));
    if (!filePath.startsWith(root)) {
      res.writeHead(403);
      res.end("forbidden");
      return;
    }

    const body = await readFile(filePath);
    res.writeHead(200, {
      "Content-Type": contentTypes.get(extname(filePath)) || "application/octet-stream",
      "Cache-Control": "no-store",
    });
    res.end(body);
  } catch (error) {
    res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
    res.end(`not found: ${String(error)}`);
  }
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`http://127.0.0.1:${port}/\n`);
});
