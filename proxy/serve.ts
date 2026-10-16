// The same handler as api/proxy.ts, served by node on this machine instead of
// a hosted function. CI starts one per job (ci.yml) and gives Nix its address
// as a substituter, so a build reads the project's cache in GHCR whether or
// not a public deployment is up. It needs nothing but GHCR_REPOSITORY, since
// the packages are public and their pull tokens anonymous.
//
//   sh build.sh && node --experimental-strip-types serve.ts 127.0.0.1:8484
//
// Only the address given is listened on, so pass a loopback one: there is no
// /ping store behind this, and nothing here is meant to be reachable from
// anywhere else.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { Readable } from "node:stream";
import { bind, handler } from "./lib/handler.ts";

const [host, port] = (process.argv[2] || "127.0.0.1:8484").split(":");
const module = await WebAssembly.compile(
  await readFile(new URL("./api/losos_proxy.wasm", import.meta.url)),
);
const handle = handler(bind(await WebAssembly.instantiate(module, {})), {
  GHCR_REPOSITORY: process.env.GHCR_REPOSITORY,
  GHCR_TOKEN: process.env.GHCR_TOKEN,
  GHCR_USERNAME: process.env.GHCR_USERNAME,
});

createServer(async (incoming, outgoing) => {
  try {
    const headers = new Headers();
    for (const [name, value] of Object.entries(incoming.headers)) {
      if (typeof value === "string") headers.set(name, value);
    }
    const response = await handle(
      new Request(`http://${host}:${port}${incoming.url}`, { method: incoming.method, headers }),
    );
    outgoing.writeHead(response.status, Object.fromEntries(response.headers));
    if (response.body) {
      Readable.fromWeb(response.body as import("node:stream/web").ReadableStream).pipe(outgoing);
    } else {
      outgoing.end();
    }
  } catch (error) {
    // A registry that drops the connection mid-answer; Nix treats a 502 as
    // a miss and builds the path instead.
    outgoing.writeHead(502, { "content-type": "text/plain; charset=utf-8" });
    outgoing.end(`${(error as Error).message}\n`);
  }
}).listen(Number(port), host, () => {
  console.log(`Serving ghcr.io/${process.env.GHCR_REPOSITORY} at http://${host}:${port}/`);
});
