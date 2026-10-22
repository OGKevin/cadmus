import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import plugin from "../plugins/cadmus.kobo/index.ts";
import {
  INSTALL_DIR,
  cargoFeatures,
  hostSchema,
  rsyncArgs,
  run,
} from "../plugins/cadmus.kobo/common.ts";
import { createDeployTool } from "../plugins/cadmus.kobo/deploy.ts";
import { RESTART_SCRIPT } from "../plugins/cadmus.kobo/restart.ts";

type RegisteredTool = {
  name: string;
  description: string;
  input: {
    required?: string[];
    additionalProperties?: boolean;
    properties?: Record<string, unknown>;
  };
  execute: (input: unknown, context: unknown) => Promise<{ content: string }>;
};

async function setup(): Promise<{
  tools: RegisteredTool[];
  namespaces: string[];
}> {
  const tools: RegisteredTool[] = [];
  const namespaces: string[] = [];
  const context = {
    location: { directory: process.cwd() },
    tool: {
      transform: async (
        callback: (editor: {
          namespace: (namespace: { name: string }) => void;
          add: (tool: RegisteredTool) => void;
        }) => void,
      ) => {
        callback({
          namespace: (namespace) => namespaces.push(namespace.name),
          add: (tool) => tools.push(tool),
        });
      },
    },
  };
  await plugin.setup(context as unknown as Parameters<typeof plugin.setup>[0]);
  return { tools, namespaces };
}

function toolByName(tools: RegisteredTool[], name: string) {
  const tool = tools.find((candidate) => candidate.name === name);
  assert.ok(tool, `missing tool ${name}`);
  return tool;
}

function toolContext(signal = AbortSignal.timeout(5_000)) {
  return {
    signal,
    progress: async () => {},
  };
}

function writeStub(dir: string, name: string, body: string) {
  const path = join(dir, name);
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

function withPath<T>(dir: string, fn: () => T): T {
  const previous = process.env.PATH;
  process.env.PATH = `${dir}:${previous ?? ""}`;
  try {
    return fn();
  } finally {
    process.env.PATH = previous;
  }
}

async function withPathAsync<T>(dir: string, fn: () => Promise<T>): Promise<T> {
  const previous = process.env.PATH;
  process.env.PATH = `${dir}:${previous ?? ""}`;
  try {
    return await fn();
  } finally {
    process.env.PATH = previous;
  }
}

function stubPath(dir: string) {
  return `${dir}:${process.env.PATH ?? ""}`;
}

function deployTool(root: string): RegisteredTool {
  return createDeployTool(root) as unknown as RegisteredTool;
}

test("registers the deploy and restart tools", async () => {
  const { tools, namespaces } = await setup();
  assert.equal(toolByName(tools, "deploy").name, "deploy");
  assert.equal(toolByName(tools, "restart").name, "restart");
  assert.deepEqual(namespaces, ["cadmus.kobo"]);
});

test("both tools require the device address and reject extra fields", async () => {
  const { tools } = await setup();
  for (const name of ["deploy", "restart"]) {
    const tool = toolByName(tools, name);
    assert.deepEqual(tool.input.required, ["ip"]);
    assert.equal(tool.input.additionalProperties, false);
  }
});

test("the deploy tool takes optional extra features", async () => {
  const { tools } = await setup();
  const deploy = toolByName(tools, "deploy");
  assert.equal(typeof deploy.input.properties?.features, "object");
  assert.deepEqual(deploy.input.required, ["ip"]);
});

test("cargoFeatures always keeps test,kobo first", () => {
  assert.equal(cargoFeatures(), "test,kobo");
  assert.equal(cargoFeatures("tracing"), "test,kobo,tracing");
  assert.equal(cargoFeatures("test,kobo,tracing"), "test,kobo,tracing");
  assert.equal(cargoFeatures("kobo,tracing,test"), "test,kobo,tracing");
  assert.throws(() => cargoFeatures("tracing; reboot"), /invalid feature/);
});

test("rsync uses batch-mode ssh and the test install", () => {
  assert.deepEqual(rsyncArgs("192.168.1.128", ["dist/cadmus", "dist/libs"]), [
    "-rc",
    "-e",
    "ssh -o BatchMode=yes -o ConnectTimeout=15",
    "dist/cadmus",
    "dist/libs",
    `root@192.168.1.128:${INSTALL_DIR}`,
  ]);
});

test("rsync brackets an IPv6 host", () => {
  assert.deepEqual(rsyncArgs("fe80::1", ["dist/cadmus"]), [
    "-rc",
    "-e",
    "ssh -o BatchMode=yes -o ConnectTimeout=15",
    "dist/cadmus",
    `root@[fe80::1]:${INSTALL_DIR}`,
  ]);
  assert.deepEqual(rsyncArgs("fe80::1%wlan0", ["dist/cadmus"]), [
    "-rc",
    "-e",
    "ssh -o BatchMode=yes -o ConnectTimeout=15",
    "dist/cadmus",
    `root@[fe80::1%wlan0]:${INSTALL_DIR}`,
  ]);
});

test("the deploy tool refuses a host with shell metacharacters", async () => {
  const { tools } = await setup();
  await assert.rejects(
    () =>
      toolByName(tools, "deploy").execute(
        { ip: "192.168.1.128; reboot" },
        toolContext(),
      ),
    /invalid host/,
  );
});

test("the restart tool refuses a host with shell metacharacters", async () => {
  const { tools } = await setup();
  await assert.rejects(
    () =>
      toolByName(tools, "restart").execute({ ip: "$(reboot)" }, toolContext()),
    /invalid host/,
  );
});

test("a plain address or hostname is accepted", () => {
  assert.doesNotThrow(() => hostSchema.parse("192.168.1.128"));
  assert.doesNotThrow(() => hostSchema.parse("kobo.local"));
  assert.doesNotThrow(() => hostSchema.parse("fe80::1%wlan0"));
});

test("zod rejects extra fields on execute", async () => {
  const { tools } = await setup();
  await assert.rejects(
    () =>
      toolByName(tools, "restart").execute(
        { ip: "192.168.1.128", extra: true },
        toolContext(),
      ),
    /unrecognized key/i,
  );
});

test("the restart script is valid shell", () => {
  execFileSync("sh", ["-n"], { input: RESTART_SCRIPT });
});

test("the restart script restarts a process in the test install", () => {
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  writeStub(
    dir,
    "pidof",
    'if [ -f "$STUB_DIR/killed" ]; then echo 200; else echo 100; fi',
  );
  writeStub(dir, "readlink", "echo /mnt/onboard/.adds/cadmus-tst");
  writeStub(dir, "kill", 'touch "$STUB_DIR/killed"');
  writeStub(dir, "sleep", ":");
  const result = withPath(dir, () =>
    execFileSync("sh", ["-c", RESTART_SCRIPT], {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: stubPath(dir),
        STUB_DIR: dir,
      },
    }),
  );
  assert.match(result, /RESULT: restarted/);
  assert.match(result, /100 -> 200/);
});

test("the restart script ignores a cadmus process in another install", () => {
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  writeStub(dir, "pidof", "echo 9");
  writeStub(dir, "readlink", "echo /mnt/onboard/.adds/cadmus");
  writeStub(dir, "kill", "exit 1");
  writeStub(dir, "sleep", ":");
  try {
    withPath(dir, () =>
      execFileSync("sh", ["-c", RESTART_SCRIPT], {
        encoding: "utf8",
        env: { ...process.env, PATH: stubPath(dir) },
      }),
    );
    assert.fail("expected exit 1");
  } catch (error) {
    assert.equal((error as { status?: number }).status, 1);
  }
});

test("the restart script fails when the pid does not change", () => {
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  writeStub(dir, "pidof", "echo 7");
  writeStub(dir, "readlink", "echo /mnt/onboard/.adds/cadmus-tst");
  writeStub(dir, "kill", ":");
  writeStub(dir, "sleep", ":");
  try {
    withPath(dir, () =>
      execFileSync("sh", ["-c", RESTART_SCRIPT], {
        encoding: "utf8",
        env: { ...process.env, PATH: stubPath(dir) },
      }),
    );
    assert.fail("expected exit 2");
  } catch (error) {
    assert.equal((error as { status?: number }).status, 2);
  }
});

test("restart reports each remote exit code", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  const log = join(dir, "ssh.log");
  writeStub(
    dir,
    "ssh",
    [
      'printf "%s\n" "$@" >> "$STUB_LOG"',
      'code="${SSH_EXIT:-0}"',
      'echo "remote $code"',
      'exit "$code"',
    ].join("\n"),
  );
  const { tools } = await setup();
  const restart = toolByName(tools, "restart");
  await withPathAsync(dir, async () => {
    process.env.STUB_LOG = log;
    process.env.SSH_EXIT = "0";
    let text = await restart.execute({ ip: "192.168.1.128" }, toolContext());
    assert.match(text.content, /Restart on 192\.168\.1\.128/);

    process.env.SSH_EXIT = "1";
    text = await restart.execute({ ip: "192.168.1.128" }, toolContext());
    assert.match(text.content, /not running/);

    process.env.SSH_EXIT = "2";
    text = await restart.execute({ ip: "192.168.1.128" }, toolContext());
    assert.match(text.content, /did not restart/);

    process.env.SSH_EXIT = "255";
    text = await restart.execute({ ip: "192.168.1.128" }, toolContext());
    assert.match(text.content, /ssh to 192\.168\.1\.128 failed \(exit 255\)/);
    delete process.env.STUB_LOG;
    delete process.env.SSH_EXIT;
  });
});

test("deploy runs cadmuscli then rsync with test,kobo", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  mkdirSync(join(root, "dist"));
  writeFileSync(join(root, "dist", "cadmus"), "binary\n");
  const log = join(root, "argv.log");
  writeStub(bin, "cadmuscli", 'printf "cadmuscli %s\n" "$*" >> "$STUB_LOG"');
  writeStub(bin, "rsync", 'printf "rsync %s\n" "$*" >> "$STUB_LOG"');
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  const deploy = deployTool(root);
  try {
    const text = await withPathAsync(bin, () =>
      deploy.execute({ ip: "192.168.1.128" }, toolContext()),
    );
    assert.match(text.content, /build \(test,kobo\): ok/);
    assert.match(text.content, /dist: ok/);
    assert.match(text.content, /rsync \(1 entries/);
    assert.match(text.content, /Deployed to 192\.168\.1\.128/);
    assert.doesNotMatch(text.content, /Finished/);
    const argv = execFileSync("cat", [log], { encoding: "utf8" });
    assert.match(argv, /cadmuscli build-kobo --features test,kobo/);
    assert.match(argv, /cadmuscli dist/);
    assert.match(argv, /rsync -rc -e /);
    assert.match(argv, /BatchMode=yes/);
    assert.doesNotMatch(argv, /tracing/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("deploy appends extra features on top of test,kobo", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  mkdirSync(join(root, "dist"));
  writeFileSync(join(root, "dist", "cadmus"), "binary\n");
  const log = join(root, "argv.log");
  writeStub(bin, "cadmuscli", 'printf "cadmuscli %s\n" "$*" >> "$STUB_LOG"');
  writeStub(bin, "rsync", 'printf "rsync %s\n" "$*" >> "$STUB_LOG"');
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  const deploy = deployTool(root);
  try {
    const text = await withPathAsync(bin, () =>
      deploy.execute(
        { ip: "192.168.1.128", features: "tracing" },
        toolContext(),
      ),
    );
    assert.match(text.content, /build \(test,kobo,tracing\): ok/);
    const argv = execFileSync("cat", [log], { encoding: "utf8" });
    assert.match(argv, /cadmuscli build-kobo --features test,kobo,tracing/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("deploy stops after a failed build", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const log = join(root, "argv.log");
  writeStub(
    bin,
    "cadmuscli",
    'printf "cadmuscli %s\n" "$*" >> "$STUB_LOG"; echo "build exploded"; exit 9',
  );
  writeStub(bin, "rsync", 'printf "rsync\n" >> "$STUB_LOG"; exit 0');
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  try {
    const text = await withPathAsync(bin, () =>
      deployTool(root).execute({ ip: "192.168.1.128" }, toolContext()),
    );
    assert.match(text.content, /build \(test,kobo\): failed \(exit 9\)/);
    assert.match(text.content, /build exploded/);
    assert.doesNotMatch(text.content, /^dist:/m);
    const argv = execFileSync("cat", [log], { encoding: "utf8" });
    assert.doesNotMatch(argv, /rsync/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("an already-aborted signal never starts a command", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  const marker = join(dir, "started");
  const bin = writeStub(dir, "mark-started", `touch "${marker}"`);
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(
    () => run(bin, [], { signal: controller.signal }),
    (error: Error) => error.name === "AbortError",
  );
  assert.equal(existsSync(marker), false);
});
