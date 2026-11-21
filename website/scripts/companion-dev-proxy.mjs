import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { readdirSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const PROXY_PORT = 3999;
const websiteRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const procedures = new Map([
  ["/cadmus.api.shared.v1.Status/", new Set(["GetStatus"])],
  [
    "/cadmus.api.shared.v1.Settings/",
    new Set(["List", "Update", "AddLibrary", "RemoveLibrary"]),
  ],
  ["/cadmus.api.shared.v1.FileOps/", new Set(["ListDirectory", "GetFile"])],
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

const downBody = {
  companion: "down",
  message:
    "No companion is running. Start the emulator with `cargo xtask run-emulator`, then turn Companion on.",
};

function discoverEmulatorPort() {
  return choosePort(cadmusListeners());
}

function procedureName(url = "") {
  const path = url.split("?")[0].replace(/\/$/, "");
  for (const [prefix, names] of procedures) {
    if (!path.startsWith(prefix)) continue;
    const name = path.slice(prefix.length);
    return names.has(name) ? name : null;
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
    for (const name of names) {
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

async function forward(port, req, body) {
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
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: req.method ?? "GET",
    headers,
    ...(req.method === "POST" ? { body } : {}),
  });
  return {
    status: response.status,
    type: response.headers.get("content-type") ?? "application/json",
    bytes: Buffer.from(await response.arrayBuffer()),
  };
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
    const name = procedureName(req.url);
    if (!isSpec && !name) {
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
      const expectedType =
        name === "GetFile" ? "application/connect+json" : "application/json";
      const gotType = req.headers["content-type"] ?? "";
      if (gotType !== expectedType) {
        refuse(
          res,
          415,
          `Wrong content-type for ${path}.`,
          expectedType,
          gotType || "(none)",
        );
        return;
      }
    }
    const body = isSpec ? Buffer.alloc(0) : await readBody(req);
    for (let attempt = 0; attempt < 2 && port; attempt += 1) {
      try {
        const upstream = await forward(port, req, body);
        note(`emulator :${port}`);
        res.writeHead(upstream.status, {
          "content-type": upstream.type,
          "content-length": upstream.bytes.length,
          "cache-control": "no-store",
        });
        res.end(upstream.bytes);
        return;
      } catch {
        port = await resolvePort(true);
      }
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
  live.close();
  stub.close();
}

function serve() {
  const handler = createHandler(discoverEmulatorPort);
  const server = createServer((req, res) => {
    handler(req, res).catch((error) => {
      console.error(error);
      if (!res.headersSent) sendJson(res, 500, {});
    });
  });
  server.on("error", (error) => {
    console.error(`companion-dev-proxy: ${error.message}`);
    process.exit(1);
  });
  server.listen(PROXY_PORT, "127.0.0.1", () => {
    console.log(`companion-dev-proxy: 127.0.0.1:${PROXY_PORT}`);
    const nextBin = join(
      websiteRoot,
      "node_modules",
      "next",
      "dist",
      "bin",
      "next",
    );
    const child = spawn(process.execPath, [nextBin, "dev"], {
      cwd: websiteRoot,
      stdio: "inherit",
    });
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
