import type { ToolContext } from "@opencode/plugin/promise/tool";
import {
  INSTALL_DIR,
  cargoFeatures,
  deployInputSchema,
  distEntries,
  report,
  resolveBin,
  rsyncArgs,
  run,
  runInShell,
  type DeployInput,
} from "./common.ts";

async function deploy(
  input: DeployInput,
  root: string,
  signal: AbortSignal,
  status: (text: string) => Promise<void>,
) {
  const features = cargoFeatures(input.features);
  const lines: string[] = [];

  await status(`building ${features} for ARM`);
  const build = await runInShell(
    root,
    ["cadmuscli", "build-kobo", "--features", features],
    { signal },
  );
  lines.push(report(`build (${features})`, build));
  if (build.code !== 0) return lines.join("\n\n");

  await status("assembling dist/");
  const dist = await runInShell(root, ["cadmuscli", "dist"], { signal });
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
      "Always includes test,kobo so the binary matches the cadmus-tst install path.",
      "Pass extra Cargo features such as tracing; they are added on top of that pair.",
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
          description:
            "Extra Cargo features to add on top of test,kobo, for example tracing",
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
        deployInputSchema.parse(input),
        root,
        context.signal,
        (text) => context.progress({ status: text }),
      );
      return { content: text };
    },
  };
}
