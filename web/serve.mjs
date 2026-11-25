// A static file server to look at the assembled page on this machine.
//
//   node web/serve.mjs [directory] [port]      (default: web/dist, 8080)
//
// It listens on 127.0.0.1 only, sends the media types a browser needs for ES
// modules and for compiling WebAssembly while it streams
// (application/wasm), and the headers of `_headers` for "/*", as Cloudflare
// Pages would. Not for anything but a local test.

import { createServer } from "node:http";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const web = dirname(fileURLToPath(import.meta.url));
const directory = resolve(process.argv[2] ?? join(web, "dist"));
const portArg = process.argv[3];
const port = portArg === undefined ? 8080 : Number(portArg);
if (!Number.isInteger(port) || port < 0 || port > 65535) {
  console.error(`Invalid port: ${portArg}`);
  process.exit(1);
}

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".csv": "text/csv; charset=utf-8",
};

/// The headers of the block "/*" of a `_headers` file.
function commonHeaders() {
  const path = join(directory, "_headers");
  const headers = {};
  if (!existsSync(path)) return headers;
  let inBlock = false;
  for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;
    if (!/^\s/.test(line)) {
      inBlock = line.trim() === "/*";
    } else if (inBlock) {
      const colon = line.indexOf(":");
      if (colon > 0) headers[line.slice(0, colon).trim()] = line.slice(colon + 1).trim();
    }
  }
  return headers;
}

if (!existsSync(join(directory, "index.html"))) {
  console.error(`${directory} has no index.html. Assemble the page first: node web/build.mjs`);
  process.exit(1);
}
const common = commonHeaders();

createServer((request, response) => {
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.writeHead(405, {
      Allow: "GET, HEAD",
      "Content-Type": "text/plain; charset=utf-8",
      "X-Content-Type-Options": "nosniff",
      ...common,
    });
    response.end("Method not allowed\n");
    return;
  }
  let path;
  try {
    path = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
  } catch {
    path = "/";
  }
  if (path.endsWith("/")) path += "index.html";
  const file = normalize(join(directory, path));
  const inside = file === directory || file.startsWith(directory + sep);
  if (!inside || path.toLowerCase() === "/_headers" || !existsSync(file) || !statSync(file).isFile()) {
    const notFound = join(directory, "404.html");
    if (existsSync(notFound) && statSync(notFound).isFile()) {
      response.writeHead(404, {
        "Content-Type": TYPES[".html"],
        "X-Content-Type-Options": "nosniff",
        ...common,
      });
      response.end(request.method === "HEAD" ? undefined : readFileSync(notFound));
    } else {
      response.writeHead(404, {
        "Content-Type": "text/plain; charset=utf-8",
        "X-Content-Type-Options": "nosniff",
        ...common,
      });
      response.end("Not found\n");
    }
    return;
  }
  response.writeHead(200, {
    "Content-Type": TYPES[extname(file)] ?? "application/octet-stream",
    "Cache-Control": "no-cache",
    "X-Content-Type-Options": "nosniff",
    ...common,
  });
  response.end(request.method === "HEAD" ? undefined : readFileSync(file));
}).listen(port, "127.0.0.1", () => {
  console.log(`http://127.0.0.1:${port}/  (${directory})`);
});
