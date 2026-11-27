import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:http";
import { Readable } from "node:stream";
import { readdirSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const websiteRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

const UNARY_TYPE = "application/json";
const STREAM_TYPE = "application/connect+json";

// Every method the Companion site calls, with the request content type it
// sends: a stream frames its messages, a unary method sends one JSON body.
const procedures = new Map([
  ["/cadmus.api.shared.v1.Status/", new Map([["GetStatus", UNARY_TYPE]])],
  [
    "/cadmus.api.shared.v1.Settings/",
    new Map([
      ["List", UNARY_TYPE],
      ["Update", UNARY_TYPE],
      ["AddLibrary", UNARY_TYPE],
      ["RemoveLibrary", UNARY_TYPE],
    ]),
  ],
  [
    "/cadmus.api.shared.v1.FileOps/",
    new Map([
      ["ListDirectory", UNARY_TYPE],
      ["GetFile", STREAM_TYPE],
    ]),
  ],
  [
    "/cadmus.api.public.v1.Logs/",
    new Map([
      ["List", UNARY_TYPE],
      ["Read", UNARY_TYPE],
      ["Summarize", STREAM_TYPE],
      ["Download", STREAM_TYPE],
    ]),
  ],
]);

const OPENAPI_PATH = "/openapi.json";

function parseListenInodes(text) {
  const sockets = new Map();
  for (const line of text.split("\n").slice(1)) {
    const parts = line.trim().split(/\s+/);
    if (parts.length < 10 || parts[3] !== "0A") continue;
    const [ipHex, portHex] = parts[1].split(":");
    if (!portHex) continue;
    sockets.set(parts[9], {
      port: Number.parseInt(portHex, 16),
      any: ipHex === "00000000",
    });
  }
  return sockets;
}

function parseLsofListeners(text) {
  const found = [];
  let pid = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("p")) {
      pid = Number(line.slice(1));
    } else if (line.startsWith("n")) {
      const colon = line.lastIndexOf(":");
      const host = line.slice(1, colon);
      found.push({
        pid,
        port: Number(line.slice(colon + 1)),
        any: host === "*" || host === "0.0.0.0" || host === "[::]",
      });
    }
  }
  return found;
}

async function probeInternal(port) {
  const response = await fetch(
    `http://127.0.0.1:${port}/cadmus.api.internal.v1.Placeholder/Ping`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
      signal: AbortSignal.timeout(500),
    },
  );
  return response.status;
}

// The Kobo binds Companion on the LAN (wildcard), where the newest cadmus
// process wins. The debug emulator binds Companion to loopback next to the
// internal listener, and debug builds accept any bearer, so auth answers
// cannot tell them apart. Only the internal listener registers
// `Placeholder/Ping`, so the listener that does not answer it is Companion.
async function choosePort(listeners, probe = probeInternal) {
  const wildcard = listeners.filter((item) => item.any);
  if (wildcard.length > 0) {
    let best = wildcard[0];
    for (const item of wildcard.slice(1)) {
      if (item.pid > best.pid) best = item;
    }
    return best.port;
  }
  const ports = [...new Set(listeners.map((item) => item.port))];
  for (const port of ports) {
    let status;
    try {
      status = await probe(port);
    } catch {
      continue;
    }
    if (status !== 200) return port;
  }
  return null;
}

function cadmusListeners() {
  let sockets;
  try {
    sockets = parseListenInodes(readFileSync("/proc/net/tcp", "utf8"));
  } catch {
    return [];
  }
  const found = [];
  let pids;
  try {
    pids = readdirSync("/proc");
  } catch {
    return [];
  }
  for (const pidText of pids) {
    if (!/^\d+$/.test(pidText)) continue;
    let comm;
    try {
      comm = readFileSync(`/proc/${pidText}/comm`, "utf8");
    } catch {
      continue;
    }
    if (comm.trim() !== "cadmus") continue;
    let fds;
    try {
      fds = readdirSync(`/proc/${pidText}/fd`);
    } catch {
      continue;
    }
    for (const fd of fds) {
      let link;
      try {
        link = readlinkSync(`/proc/${pidText}/fd/${fd}`);
      } catch {
        continue;
      }
      const match = /^socket:\[(\d+)\]$/.exec(link);
      if (!match) continue;
      const sock = sockets.get(match[1]);
      if (!sock) continue;
      found.push({
        pid: Number(pidText),
        port: sock.port,
        any: sock.any,
      });
    }
  }
  return found;
}

function darwinListeners() {
  let text;
  try {
    text = execFileSync(
      "lsof",
      ["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-c", "cadmus", "-F", "pn"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
  } catch (error) {
    if (error.status === 1 && error.stdout === "") return [];
    throw new Error(
      `companion-dev-proxy: lsof failed while discovering the emulator: ${error.message}`,
    );
  }
  return parseLsofListeners(text);
}

function emulatorListeners() {
  if (process.platform === "linux") return cadmusListeners();
  if (process.platform === "darwin") return darwinListeners();
  throw new Error(
    `companion-dev-proxy: emulator discovery is not supported on ${process.platform}; set COMPANION_API_PORT to the emulator API port`,
  );
}

function apiPortOverride(env = process.env) {
  const value = env.COMPANION_API_PORT;
  if (value === undefined || value === "") return null;
  const port = Number(value);
  if (!/^\d+$/.test(value) || port < 1 || port > 65535) {
    throw new Error(
      `companion-dev-proxy: COMPANION_API_PORT must be an integer from 1 to 65535, got "${value}"`,
    );
  }
  return port;
}

const downBody = {
  companion: "down",
  message:
    "No companion is running. Start the emulator with `cargo xtask run-emulator`, then turn Companion on.",
};

function discoverEmulatorPort() {
  const override = apiPortOverride();
  if (override !== null) return override;
  return choosePort(emulatorListeners());
}

function nextDevArgs(args) {
  const hostFlag = args.some(
    (arg) =>
      arg === "-H" || arg === "--hostname" || arg.startsWith("--hostname="),
  );
  return hostFlag ? ["dev", ...args] : ["dev", "-H", "127.0.0.1", ...args];
}

function procedureName(url = "") {
  const path = url.split("?")[0].replace(/\/$/, "");
  for (const [prefix, names] of procedures) {
    if (!path.startsWith(prefix)) continue;
    const name = path.slice(prefix.length);
    if (names.has(name)) {
      return { name, type: names.get(name) };
    }
  }
  return null;
}

function sendJson(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
  });
  res.end(body);
}

function allowedProcedures() {
  const list = [];
  for (const [prefix, names] of procedures) {
    for (const name of names.keys()) {
      list.push(`${prefix}${name}`);
    }
  }
  return list.sort();
}

function refuse(res, status, problem, expected, got) {
  console.error(
    "companion-dev-proxy: refusing to proxy a request that breaks the Connect contract.",
  );
  console.error(`  ${problem}`);
  console.error(`  got: ${got}`);
  console.error(`  expected: ${expected}`);
  sendJson(res, status, {
    error: `${problem} got: ${got}; expected: ${expected}`,
  });
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > 1_000_000) {
        reject(new Error("body too large"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

function forward(port, req, body, signal) {
  const path = (req.url ?? "/").split("?")[0];
  const headers = {
    "connect-protocol-version": req.headers["connect-protocol-version"] ?? "1",
    ...(req.headers.authorization
      ? { authorization: req.headers.authorization }
      : {}),
  };
  if (req.headers["content-type"]) {
    headers["content-type"] = req.headers["content-type"];
  }
  return fetch(`http://127.0.0.1:${port}${path}`, {
    method: req.method ?? "GET",
    headers,
    signal,
    ...(req.method === "POST" ? { body } : {}),
  });
}

function createHandler(discover) {
  const state = { port: undefined, expires: 0, mode: "" };

  async function resolvePort(force) {
    const now = Date.now();
    if (!force && state.port !== undefined && state.expires > now) {
      return state.port;
    }
    const port = await discover();
    state.port = port;
    state.expires = now + 1000;
    return port;
  }

  function note(mode) {
    if (state.mode === mode) return;
    state.mode = mode;
    console.log(`companion-dev-proxy: ${mode}`);
  }

  return async function handler(req, res) {
    const path = (req.url ?? "/").split("?")[0];
    const isSpec = path === OPENAPI_PATH;
    const procedure = procedureName(req.url);
    if (!isSpec && !procedure) {
      refuse(
        res,
        404,
        "Unknown RPC; add it to `procedures` in scripts/companion-dev-proxy.mjs if the contract grew.",
        allowedProcedures().join(", "),
        `${req.method} ${path}`,
      );
      return;
    }
    let port = await resolvePort(false);
    if (!port) {
      note("down");
      sendJson(res, 503, downBody);
      return;
    }
    if (isSpec ? req.method !== "GET" : req.method !== "POST") {
      sendJson(res, 405, {});
      return;
    }
    if (!isSpec) {
      const gotType = req.headers["content-type"] ?? "";
      if (gotType !== procedure.type) {
        refuse(
          res,
          415,
          `Wrong content-type for ${path}.`,
          procedure.type,
          gotType || "(none)",
        );
        return;
      }
    }
    const body = isSpec ? Buffer.alloc(0) : await readBody(req);
    const streaming = procedure?.type === STREAM_TYPE;
    for (let attempt = 0; attempt < 2 && port; attempt += 1) {
      const abort = new AbortController();
      let upstream;
      try {
        upstream = await forward(port, req, body, abort.signal);
      } catch {
        abort.abort();
        port = await resolvePort(true);
        continue;
      }
      note(`emulator :${port}`);
      res.on("close", () => abort.abort());
      if (streaming) {
        res.writeHead(upstream.status, {
          "content-type": upstream.headers.get("content-type") ?? STREAM_TYPE,
          "cache-control": "no-store",
        });
        if (!upstream.body) {
          res.end();
          return;
        }
        const stream = Readable.fromWeb(upstream.body);
        stream.on("error", () => res.destroy());
        res.on("close", () => stream.destroy());
        stream.pipe(res);
        return;
      }
      let bytes;
      try {
        bytes = Buffer.from(await upstream.arrayBuffer());
      } catch (error) {
        console.error(
          `companion-dev-proxy: emulator response was cut off: ${error.message}`,
        );
        sendJson(res, 502, {});
        return;
      }
      res.writeHead(upstream.status, {
        "content-type":
          upstream.headers.get("content-type") ?? "application/json",
        "content-length": bytes.length,
        "cache-control": "no-store",
      });
      res.end(bytes);
      return;
    }
    note("down");
    sendJson(res, 503, downBody);
  };
}

function listen(discover) {
  const handler = createHandler(discover);
  return createServer((req, res) => {
    handler(req, res).catch((error) => {
      console.error(error);
      if (!res.headersSent) sendJson(res, 500, {});
    });
  });
}

async function check() {
  const table = [
    "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode",
    "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0",
    "   1: 0100007F:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 999 1 0000000000000000 100 0 0 10 0",
  ].join("\n");
  const sockets = parseListenInodes(table);
  assert.equal(sockets.get("12345")?.port, 8080);
  assert.equal(sockets.get("12345")?.any, true);
  assert.equal(sockets.get("999")?.any, false);
  assert.deepEqual(
    parseLsofListeners(
      "p101\nn*:4000\np202\nn127.0.0.1:4001\nn[::1]:4002\nn[::]:4003\n",
    ),
    [
      { pid: 101, port: 4000, any: true },
      { pid: 202, port: 4001, any: false },
      { pid: 202, port: 4002, any: false },
      { pid: 202, port: 4003, any: true },
    ],
  );
  assert.equal(apiPortOverride({}), null);
  assert.equal(apiPortOverride({ COMPANION_API_PORT: "" }), null);
  assert.equal(apiPortOverride({ COMPANION_API_PORT: "3010" }), 3010);
  for (const value of ["0", "65536", "3010.5", "abc", "-1"]) {
    assert.throws(
      () => apiPortOverride({ COMPANION_API_PORT: value }),
      /COMPANION_API_PORT must be an integer from 1 to 65535/,
    );
  }
  assert.deepEqual(nextDevArgs([]), ["dev", "-H", "127.0.0.1"]);
  assert.deepEqual(nextDevArgs(["--port", "3000"]), [
    "dev",
    "-H",
    "127.0.0.1",
    "--port",
    "3000",
  ]);
  assert.deepEqual(nextDevArgs(["-H", "0.0.0.0"]), ["dev", "-H", "0.0.0.0"]);
  assert.deepEqual(nextDevArgs(["--hostname=0.0.0.0"]), [
    "dev",
    "--hostname=0.0.0.0",
  ]);
  assert.equal(
    await choosePort(
      [
        { pid: 1, port: 10, any: false },
        { pid: 4, port: 20, any: true },
        { pid: 3, port: 30, any: true },
      ],
      async () => 401,
    ),
    20,
  );
  assert.equal(await choosePort([], async () => 401), null);
  assert.equal(
    await choosePort(
      [
        { pid: 5, port: 10, any: false },
        { pid: 5, port: 30, any: false },
      ],
      async (port) => (port === 30 ? 401 : 200),
    ),
    30,
  );
  assert.equal(
    await choosePort([{ pid: 5, port: 10, any: false }], async () => 200),
    null,
  );
  assert.equal(
    await choosePort([{ pid: 5, port: 10, any: false }], async () => {
      throw new Error("connection refused");
    }),
    null,
  );

  const proxy = listen(async () => null);
  await new Promise((resolve) => proxy.listen(0, "127.0.0.1", resolve));
  const address = proxy.address();
  const status = await fetch(
    `http://127.0.0.1:${address.port}/cadmus.api.shared.v1.Status/GetStatus`,
  );
  const statusBody = await status.json();
  assert.equal(status.status, 503);
  assert.equal(statusBody.companion, "down");
  const listing = await fetch(
    `http://127.0.0.1:${address.port}/cadmus.api.shared.v1.FileOps/ListDirectory`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  const listingBody = await listing.json();
  assert.equal(listing.status, 503);
  assert.equal(listingBody.companion, "down");
  const file = await fetch(
    `http://127.0.0.1:${address.port}/cadmus.api.shared.v1.FileOps/GetFile`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  const fileBody = await file.json();
  assert.equal(file.status, 503);
  assert.equal(fileBody.companion, "down");
  const listed = await fetch(
    `http://127.0.0.1:${address.port}/cadmus.api.shared.v1.Settings/List`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(listed.status, 503);
  assert.equal(
    (await fetch(`http://127.0.0.1:${address.port}/openapi.json`)).status,
    503,
  );
  const unknown = await fetch(
    `http://127.0.0.1:${address.port}/cadmus.api.shared.v1.Settings/Bogus`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(unknown.status, 404);
  assert.match((await unknown.json()).error, /Bogus/);
  proxy.close();

  const stub = createServer((_req, res) => {
    sendJson(res, 200, { categories: [] });
  });
  await new Promise((resolve) => stub.listen(0, "127.0.0.1", resolve));
  const stubPort = stub.address().port;
  const live = listen(async () => stubPort);
  await new Promise((resolve) => live.listen(0, "127.0.0.1", resolve));
  const liveAddress = live.address();
  const forwarded = await fetch(
    `http://127.0.0.1:${liveAddress.port}/cadmus.api.shared.v1.Settings/List`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  const forwardedBody = await forwarded.json();
  assert.deepEqual(forwardedBody, { categories: [] });
  const spec = await fetch(`http://127.0.0.1:${liveAddress.port}/openapi.json`);
  assert.equal(spec.status, 200);
  assert.deepEqual(await spec.json(), { categories: [] });
  const mismatch = await fetch(
    `http://127.0.0.1:${liveAddress.port}/cadmus.api.shared.v1.FileOps/GetFile`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(mismatch.status, 415);
  assert.match((await mismatch.json()).error, /connect\+json/);
  const streamMismatch = await fetch(
    `http://127.0.0.1:${liveAddress.port}/cadmus.api.public.v1.Logs/Summarize`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(streamMismatch.status, 415);
  assert.match((await streamMismatch.json()).error, /connect\+json/);
  live.close();

  function frame(flags, text) {
    const bytes = Buffer.from(text);
    const out = Buffer.alloc(5 + bytes.length);
    out[0] = flags;
    out.writeUInt32BE(bytes.length, 1);
    bytes.copy(out, 5);
    return out;
  }

  const firstFrame = frame(0, JSON.stringify({ lines: "1" }));
  const secondFrame = frame(0, JSON.stringify({ lines: "2" }));
  const endFrame = frame(2, "{}");
  let resolveStreamingClosed;
  const streamingClosed = new Promise((resolve) => {
    resolveStreamingClosed = resolve;
  });
  const streamingStub = createServer((_req, res) => {
    res.writeHead(200, { "content-type": STREAM_TYPE });
    res.write(firstFrame);
    setTimeout(() => {
      res.write(secondFrame);
      res.end(endFrame);
    }, 10);
    res.on("finish", () => resolveStreamingClosed());
  });
  await new Promise((resolve) => streamingStub.listen(0, "127.0.0.1", resolve));
  const streamingProxy = listen(async () => streamingStub.address().port);
  await new Promise((resolve) =>
    streamingProxy.listen(0, "127.0.0.1", resolve),
  );
  const streamed = await fetch(
    `http://127.0.0.1:${streamingProxy.address().port}/cadmus.api.public.v1.Logs/Summarize`,
    {
      method: "POST",
      headers: { "content-type": STREAM_TYPE },
      body: Buffer.alloc(0),
    },
  );
  assert.equal(streamed.status, 200);
  assert.equal(streamed.headers.get("content-length"), null);
  assert.deepEqual(
    Buffer.from(await streamed.arrayBuffer()),
    Buffer.concat([firstFrame, secondFrame, endFrame]),
  );
  await streamingClosed;
  streamingProxy.close();
  streamingStub.close();

  let resolveClientClosed;
  const clientClosed = new Promise((resolve) => {
    resolveClientClosed = resolve;
  });
  const endlessStub = createServer((_req, res) => {
    res.writeHead(200, { "content-type": STREAM_TYPE });
    res.write(firstFrame);
    res.on("close", () => resolveClientClosed());
  });
  await new Promise((resolve) => endlessStub.listen(0, "127.0.0.1", resolve));
  const endlessProxy = listen(async () => endlessStub.address().port);
  await new Promise((resolve) => endlessProxy.listen(0, "127.0.0.1", resolve));
  const abort = new AbortController();
  const endless = await fetch(
    `http://127.0.0.1:${endlessProxy.address().port}/cadmus.api.public.v1.Logs/Download`,
    {
      method: "POST",
      headers: { "content-type": STREAM_TYPE },
      body: Buffer.alloc(0),
      signal: abort.signal,
    },
  );
  const reader = endless.body.getReader();
  const chunk = await reader.read();
  assert.equal(
    Buffer.from(chunk.value).subarray(0, firstFrame.length).equals(firstFrame),
    true,
  );
  abort.abort();
  await Promise.race([
    clientClosed,
    new Promise((_, reject) =>
      setTimeout(
        () => reject(new Error("client abort did not reach the emulator")),
        2000,
      ),
    ),
  ]);
  endlessProxy.close();
  endlessStub.close();

  const closed = createServer();
  await new Promise((resolve) => closed.listen(0, "127.0.0.1", resolve));
  const closedPort = closed.address().port;
  await new Promise((resolve) => closed.close(resolve));
  let discoveries = 0;
  const failover = listen(async () => {
    discoveries += 1;
    return discoveries === 1 ? closedPort : stubPort;
  });
  await new Promise((resolve) => failover.listen(0, "127.0.0.1", resolve));
  const recovered = await fetch(
    `http://127.0.0.1:${failover.address().port}/cadmus.api.shared.v1.Settings/List`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(recovered.status, 200);
  assert.equal(discoveries, 2);
  failover.close();
  stub.close();

  let addLibraryCalls = 0;
  const truncating = createServer((req, res) => {
    addLibraryCalls += 1;
    res.writeHead(200, {
      "content-type": "application/json",
      "content-length": 100,
    });
    res.write('{"categories"');
    setTimeout(() => req.socket.destroy(), 10);
  });
  await new Promise((resolve) => truncating.listen(0, "127.0.0.1", resolve));
  const truncatingPort = truncating.address().port;
  const truncated = listen(async () => truncatingPort);
  await new Promise((resolve) => truncated.listen(0, "127.0.0.1", resolve));
  const cutOff = await fetch(
    `http://127.0.0.1:${truncated.address().port}/cadmus.api.shared.v1.Settings/AddLibrary`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    },
  );
  assert.equal(cutOff.status, 502);
  assert.equal(addLibraryCalls, 1);
  truncated.close();
  truncating.close();
}

function serve() {
  apiPortOverride();
  const server = listen(discoverEmulatorPort);
  server.on("error", (error) => {
    console.error(`companion-dev-proxy: ${error.message}`);
    process.exit(1);
  });
  server.listen(0, "127.0.0.1", () => {
    const origin = `http://127.0.0.1:${server.address().port}`;
    console.log(`companion-dev-proxy: ${origin}`);
    const nextBin = join(
      websiteRoot,
      "node_modules",
      "next",
      "dist",
      "bin",
      "next",
    );
    const child = spawn(
      process.execPath,
      [nextBin, ...nextDevArgs(process.argv.slice(2))],
      {
        cwd: websiteRoot,
        env: { ...process.env, COMPANION_PROXY_ORIGIN: origin },
        stdio: "inherit",
      },
    );
    const shutdown = () => {
      child.kill("SIGTERM");
      server.close();
    };
    process.on("SIGINT", shutdown);
    process.on("SIGTERM", shutdown);
    child.on("exit", (code) => {
      server.close();
      process.exit(code ?? 0);
    });
  });
}

const isMain = (() => {
  const entry = process.argv[1];
  if (!entry) return false;
  try {
    return (
      realpathSync(resolve(entry)) ===
      realpathSync(fileURLToPath(import.meta.url))
    );
  } catch {
    return false;
  }
})();

if (isMain && process.argv.includes("--check")) {
  check().catch((error) => {
    console.error(error);
    process.exit(1);
  });
} else if (isMain) {
  serve();
}
