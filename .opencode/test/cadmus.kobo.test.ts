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
  deployArgs,
  deployInputSchema,
  hostSchema,
  resolveBin,
  restartArgs,
  restartInputSchema,
  run,
} from "../plugins/cadmus.kobo/common.ts";
import { createDeployTool } from "../plugins/cadmus.kobo/deploy.ts";
import { createRestartTool } from "../plugins/cadmus.kobo/restart.ts";

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
  assert.ok(tool, `tool ${name} is not registered`);
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

async function withPathAsync<T>(dir: string, fn: () => Promise<T>): Promise<T> {
  const previous = process.env.PATH;
  process.env.PATH = `${dir}:${previous ?? ""}`;
  try {
    return await fn();
  } finally {
    process.env.PATH = previous;
  }
}

function deployTool(root: string): RegisteredTool {
  return createDeployTool(root) as unknown as RegisteredTool;
}

function restartTool(root: string): RegisteredTool {
  return createRestartTool(root) as unknown as RegisteredTool;
}

test("registers the deploy and restart tools", async () => {
  const { tools, namespaces } = await setup();
  assert.equal(toolByName(tools, "deploy").name, "deploy");
  assert.equal(toolByName(tools, "restart").name, "restart");
  assert.deepEqual(namespaces, ["cadmus.kobo"]);
});

test("the device address is optional and extra fields are rejected", async () => {
  const { tools } = await setup();
  const dir = mkdtempSync(join(tmpdir(), "cadmus-kobo-"));
  writeStub(dir, "cadmuscli", "exit 99");
  assert.doesNotThrow(() => deployInputSchema.parse({}));
  assert.doesNotThrow(() => restartInputSchema.parse({}));
  await withPathAsync(dir, async () => {
    for (const name of ["deploy", "restart"]) {
      const tool = toolByName(tools, name);
      assert.deepEqual(tool.input.required, []);
      assert.equal(tool.input.additionalProperties, false);
      await assert.rejects(
        () => tool.execute({ ip: "192.168.1.128", extra: true }, toolContext()),
        /unrecognized key/i,
      );
    }
  });
});

test("tool schemas mirror the zod shapes", () => {
  const deploy = deployTool(process.cwd());
  const restart = restartTool(process.cwd());

  assert.deepEqual(
    Object.keys(deploy.input.properties ?? {}),
    Object.keys(deployInputSchema.shape),
  );
  assert.deepEqual(
    Object.keys(restart.input.properties ?? {}),
    Object.keys(restartInputSchema.shape),
  );
  assert.deepEqual(deploy.input.required, []);
  assert.deepEqual(restart.input.required, []);
});

test("deploy argv is minimal without optional flags", () => {
  assert.deepEqual(deployArgs({}), ["kobo", "deploy"]);
  assert.deepEqual(deployArgs({ ip: "192.168.1.128" }), [
    "kobo",
    "deploy",
    "--ip",
    "192.168.1.128",
  ]);
  assert.deepEqual(
    deployArgs({ identitiesOnly: false, acceptNew: false, restart: false }),
    ["kobo", "deploy"],
  );
});

test("deploy argv forwards features, restart, and ssh options", () => {
  assert.deepEqual(
    deployArgs({
      ip: "kobo.local",
      features: "tracing",
      restart: true,
      port: 2222,
      user: "root",
      identity: "/keys/id_ed25519",
      identitiesOnly: true,
      knownHosts: "/tmp/known_hosts",
      acceptNew: true,
      sshConfig: "/tmp/ssh_config",
    }),
    [
      "kobo",
      "deploy",
      "--ip",
      "kobo.local",
      "--features",
      "tracing",
      "--restart",
      "--port",
      "2222",
      "--user",
      "root",
      "--identity",
      "/keys/id_ed25519",
      "--identities-only",
      "--known-hosts",
      "/tmp/known_hosts",
      "--accept-new",
      "--ssh-config",
      "/tmp/ssh_config",
    ],
  );
});

test("restart argv forwards ssh options", () => {
  assert.deepEqual(restartArgs({}), ["kobo", "restart"]);
  assert.deepEqual(restartArgs({ ip: "192.168.1.128" }), [
    "kobo",
    "restart",
    "--ip",
    "192.168.1.128",
  ]);
  assert.deepEqual(restartArgs({ port: 2222 }), [
    "kobo",
    "restart",
    "--port",
    "2222",
  ]);
  assert.deepEqual(restartArgs({ ip: "192.168.1.128", port: 2222 }), [
    "kobo",
    "restart",
    "--ip",
    "192.168.1.128",
    "--port",
    "2222",
  ]);
  assert.deepEqual(restartArgs({ ip: "192.168.1.128", acceptNew: true }), [
    "kobo",
    "restart",
    "--ip",
    "192.168.1.128",
    "--accept-new",
  ]);
  assert.deepEqual(restartArgs({ ip: "192.168.1.128", identitiesOnly: true }), [
    "kobo",
    "restart",
    "--ip",
    "192.168.1.128",
    "--identities-only",
  ]);
  assert.deepEqual(
    restartArgs({ ip: "192.168.1.128", identitiesOnly: false }),
    ["kobo", "restart", "--ip", "192.168.1.128"],
  );
});

test("schemas accept the documented fields and reject unknown ones", () => {
  assert.doesNotThrow(() =>
    deployInputSchema.parse({
      ip: "kobo.local",
      features: "tracing",
      restart: true,
      port: 2222,
      user: "root",
      identity: "/keys/id_ed25519",
      identitiesOnly: true,
      knownHosts: "/tmp/known_hosts",
      acceptNew: true,
      sshConfig: "/tmp/ssh_config",
    }),
  );
  assert.doesNotThrow(() =>
    restartInputSchema.parse({ ip: "kobo.local", acceptNew: true }),
  );
  assert.doesNotThrow(() =>
    restartInputSchema.parse({ ip: "kobo.local", identitiesOnly: false }),
  );
  assert.throws(
    () => restartInputSchema.parse({ ip: "kobo.local", features: "tracing" }),
    /unrecognized key/i,
  );
});

test("host names reject shell metacharacters", () => {
  assert.doesNotThrow(() => hostSchema.parse("192.168.1.128"));
  assert.doesNotThrow(() => hostSchema.parse("kobo.local"));
  assert.doesNotThrow(() => hostSchema.parse("fe80::1%wlan0"));
  assert.throws(() => hostSchema.parse("$(reboot)"));
  assert.throws(() => hostSchema.parse("kobo; reboot"));
});

test("the deploy tool runs cadmuscli kobo deploy", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const log = join(root, "argv.log");
  writeStub(
    bin,
    "cadmuscli",
    'printf "%s\\n" "$@" >> "$STUB_LOG"; echo "Uploaded 7 files"',
  );
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  try {
    const text = await withPathAsync(bin, () =>
      deployTool(root).execute(
        { ip: "192.168.1.128", features: "trac ing", restart: true },
        toolContext(),
      ),
    );
    assert.match(text.content, /Uploaded 7 files/);
    const argv = execFileSync("cat", [log], { encoding: "utf8" })
      .trim()
      .split("\n");
    assert.deepEqual(argv, [
      "kobo",
      "deploy",
      "--ip",
      "192.168.1.128",
      "--features",
      "trac ing",
      "--restart",
    ]);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("the deploy tool omits --ip and reports a sane status without one", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const log = join(root, "argv.log");
  writeStub(
    bin,
    "cadmuscli",
    'printf "%s\\n" "$@" >> "$STUB_LOG"; echo "Uploaded 1 file"',
  );
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  const statuses: string[] = [];
  const context = {
    signal: AbortSignal.timeout(5_000),
    progress: async (update: { status: string }) => {
      statuses.push(update.status);
    },
  };
  try {
    const text = await withPathAsync(bin, () =>
      deployTool(root).execute({}, context),
    );
    assert.match(text.content, /Uploaded 1 file/);
    const argv = execFileSync("cat", [log], { encoding: "utf8" })
      .trim()
      .split("\n");
    assert.deepEqual(argv, ["kobo", "deploy"]);
    assert.equal(statuses.length, 1);
    assert.doesNotMatch(statuses[0] ?? "", /undefined/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("the deploy tool surfaces failures", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-deploy-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  writeStub(bin, "cadmuscli", 'echo "build exploded" >&2; exit 9');
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  try {
    const text = await withPathAsync(bin, () =>
      deployTool(root).execute({ ip: "192.168.1.128" }, toolContext()),
    );
    assert.match(text.content, /deploy: failed \(exit 9\)/);
    assert.match(text.content, /build exploded/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
  }
});

test("the restart tool runs cadmuscli kobo restart", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-restart-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const log = join(root, "argv.log");
  writeStub(
    bin,
    "cadmuscli",
    'printf "%s\\n" "$@" >> "$STUB_LOG"; echo "Restart on 192.168.1.128:"',
  );
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  process.env.STUB_LOG = log;
  try {
    const text = await withPathAsync(bin, () =>
      restartTool(root).execute(
        { ip: "192.168.1.128", acceptNew: true },
        toolContext(),
      ),
    );
    assert.match(text.content, /Restart on 192\.168\.1\.128/);
    const argv = execFileSync("cat", [log], { encoding: "utf8" })
      .trim()
      .split("\n");
    assert.deepEqual(argv, [
      "kobo",
      "restart",
      "--ip",
      "192.168.1.128",
      "--accept-new",
    ]);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
    delete process.env.STUB_LOG;
  }
});

test("the restart tool surfaces failures", async () => {
  const root = mkdtempSync(join(tmpdir(), "cadmus-restart-"));
  const bin = join(root, "bin");
  mkdirSync(bin);
  writeStub(
    bin,
    "cadmuscli",
    'echo "Cadmus is not running on 192.168.1.128." >&2; exit 1',
  );
  const previousRoot = process.env.DEVENV_ROOT;
  process.env.DEVENV_ROOT = root;
  try {
    const text = await withPathAsync(bin, () =>
      restartTool(root).execute({ ip: "192.168.1.128" }, toolContext()),
    );
    assert.match(text.content, /restart: failed \(exit 1\)/);
    assert.match(text.content, /not running/);
  } finally {
    process.env.DEVENV_ROOT = previousRoot;
  }
});

test("child processes keep the fallback bin dirs on PATH", async () => {
  const previous = process.env.PATH;
  process.env.PATH = "";
  try {
    const result = await run(resolveBin("sh"), ["-c", 'printf %s "$PATH"']);
    assert.equal(result.code, 0);
    for (const dir of result.output.split(":")) {
      assert.ok(dir.length > 0, "PATH has an empty entry");
    }
    assert.ok(
      result.output.includes("/run/current-system/sw/bin") ||
        result.output.includes("/usr/bin"),
      `fallback dirs missing from PATH: ${result.output}`,
    );
  } finally {
    process.env.PATH = previous;
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
