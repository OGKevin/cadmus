import type { ToolContext } from "@opencode/plugin/promise/tool";
import {
  DEFAULT_FEATURES,
  INSTALL_DIR,
  assertHost,
  buildArgs,
  distArgs,
  distEntries,
  report,
  resolveBin,
  rsyncArgs,
  run,
  runInShell,
} from "./common.ts";

type DeployInput = {
  ip: string;
  features?: string;
};

async function deploy(
  input: DeployInput,
  root: string,
  signal: AbortSignal,
  status: (text: string) => Promise<void>,
) {
  assertHost(input.ip);
  const features = input.features ?? DEFAULT_FEATURES;
  const lines: string[] = [];

  await status(`building ${features} for ARM`);
  const build = await runInShell(root, ["cadmuscli", ...buildArgs(features)], {
    signal,
  });
  lines.push(report(`build (${features})`, build));
  if (build.code !== 0) return lines.join("\n\n");

  await status("assembling dist/");
  const dist = await runInShell(root, ["cadmuscli", ...distArgs()], { signal });
  lines.push(report("dist", dist));
  if (dist.code !== 0) return lines.join("\n\n");

  await status(`rsync to ${input.ip}`);
  const files = distEntries(root);
  const rsync = await run(resolveBin("rsync"), rsyncArgs(input.ip, files), {
    cwd: root,
    signal,
  });
  lines.push(
    report(`rsync (${files.length} entries to ${INSTALL_DIR})`, rsync),
  );
  if (rsync.code !== 0) return lines.join("\n\n");

  lines.push(
    `Deployed to ${input.ip}. The running process still uses the old binary until Cadmus restarts.`,
  );
  return lines.join("\n\n");
}

export function createDeployTool(root: string) {
  return {
    name: "deploy",
    description: [
      "Build Cadmus for Kobo (ARM), assemble the distribution and rsync it to a Kobo over SSH.",
      "Use this to install the current working tree on a device.",
      "The device keeps running the old binary until Cadmus is restarted with the restart tool.",
    ].join(" "),
    input: {
      type: "object",
      properties: {
        ip: {
          type: "string",
          description: "IP address of the Kobo, for example 192.168.1.128",
        },
        features: {
          type: "string",
          description: `Cargo features for the ARM build (default ${DEFAULT_FEATURES})`,
        },
      },
      required: ["ip"],
      additionalProperties: false,
    },
    options: {
      namespace: "cadmus.kobo",
      permission: "cadmus.kobo.deploy",
    },
    execute: async (input: unknown, context: ToolContext) => {
      const text = await deploy(
        input as DeployInput,
        root,
        context.signal,
        (text) => context.progress({ status: text }),
      );
      return { content: text };
    },
  };
}
