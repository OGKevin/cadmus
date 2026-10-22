import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { test } from "node:test";

import plugin from "../plugins/cadmus.kobo/index.ts";
import {
  DEFAULT_FEATURES,
  INSTALL_DIR,
  assertHost,
  buildArgs,
  distArgs,
  rsyncArgs,
} from "../plugins/cadmus.kobo/common.ts";
import { RESTART_SCRIPT } from "../plugins/cadmus.kobo/restart.ts";

type RegisteredTool = {
  name: string;
  description: string;
  input: { required?: string[]; additionalProperties?: boolean };
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

function toolContext() {
  return {
    signal: AbortSignal.timeout(5_000),
    progress: async () => {},
  };
}

test("registers the deploy and restart tools", async () => {
  const { tools, namespaces } = await setup();
  assert.deepEqual(
    tools.map((tool) => tool.name),
    ["deploy", "restart"],
  );
  assert.deepEqual(namespaces, ["cadmus.kobo"]);
});

test("both tools require the device address", async () => {
  const { tools } = await setup();
  for (const tool of tools) {
    assert.deepEqual(tool.input.required, ["ip"]);
    assert.equal(tool.input.additionalProperties, false);
  }
});

test("the deploy tool builds the ARM binary with the requested features", () => {
  assert.deepEqual(buildArgs("test,kobo"), [
    "build-kobo",
    "--features",
    "test,kobo",
  ]);
  assert.deepEqual(buildArgs(DEFAULT_FEATURES), [
    "build-kobo",
    "--features",
    "test,kobo",
  ]);
});

test("the deploy tool assembles the distribution", () => {
  assert.deepEqual(distArgs(), ["dist"]);
});

test("the deploy tool rsyncs to the test install", () => {
  const args = rsyncArgs("192.168.1.128", ["dist/cadmus", "dist/libs"]);
  assert.deepEqual(args, [
    "-Prc",
    "dist/cadmus",
    "dist/libs",
    `root@192.168.1.128:${INSTALL_DIR}`,
  ]);
  assert.match(INSTALL_DIR, /^\/mnt\/onboard\/\.adds\/cadmus-tst$/);
});

test("the deploy tool refuses a host with shell metacharacters", async () => {
  const { tools } = await setup();
  const deploy = tools[0]!;
  await assert.rejects(
    () => deploy.execute({ ip: "192.168.1.128; reboot" }, toolContext()),
    /invalid host/,
  );
});

test("the restart tool refuses a host with shell metacharacters", async () => {
  const { tools } = await setup();
  const restart = tools[1]!;
  await assert.rejects(
    () => restart.execute({ ip: "$(reboot)" }, toolContext()),
    /invalid host/,
  );
});

test("a plain address or hostname is accepted", () => {
  assert.doesNotThrow(() => assertHost("192.168.1.128"));
  assert.doesNotThrow(() => assertHost("kobo.local"));
  assert.doesNotThrow(() => assertHost("fe80::1%wlan0"));
});

test("the restart script is valid shell", () => {
  execFileSync("sh", ["-n"], { input: RESTART_SCRIPT });
});

test("the restart script asks Cadmus to restart", () => {
  assert.match(RESTART_SCRIPT, /kill -USR1/);
  assert.match(RESTART_SCRIPT, /pidof cadmus/);
});
