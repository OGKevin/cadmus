import type { ToolContext } from "@opencode/plugin/promise/tool";
import {
  deployArgs,
  deployInputSchema,
  report,
  runInShell,
  type DeployInput,
} from "./common.ts";

async function deploy(
  input: DeployInput,
  root: string,
  signal: AbortSignal,
): Promise<string> {
  const result = await runInShell(root, ["cadmuscli", ...deployArgs(input)], {
    signal,
  });
  if (result.code === 0) return result.output.trim();
  return report("deploy", result);
}

export function createDeployTool(root: string) {
  return {
    name: "deploy",
    description: [
      "Build Cadmus for Kobo (ARM), assemble the distribution and upload it to a Kobo over SSH.",
      "Use this to install the current working tree on a device.",
      "Always includes test,kobo so the binary matches the cadmus-tst install path.",
      "Pass extra Cargo features such as tracing; they are added on top of that pair.",
      "Set restart to relaunch Cadmus in the same call; otherwise the device keeps running the old binary until the restart tool is used.",
    ].join(" "),
    input: {
      type: "object",
      properties: {
        ip: {
          type: "string",
          description:
            "IP address or ~/.ssh/config alias of the Kobo, for example 192.168.1.128. Defaults to the KOBO_IP environment variable.",
        },
        features: {
          type: "string",
          description:
            "Extra Cargo features to add on top of test,kobo, for example tracing",
        },
        restart: {
          type: "boolean",
          description: "Restart Cadmus on the device after the upload",
        },
        port: {
          type: "number",
          description: "SSH port (default: ssh_config, else 22)",
        },
        user: {
          type: "string",
          description: "SSH user (default: ssh_config, else root)",
        },
        identity: {
          type: "string",
          description:
            "Private key file (default: ssh_config IdentityFile, then the usual ~/.ssh keys)",
        },
        knownHosts: {
          type: "string",
          description:
            "known_hosts file (default: ssh_config UserKnownHostsFile, else ~/.ssh/known_hosts)",
        },
        acceptNew: {
          type: "boolean",
          description:
            "Learn an unknown host key on first connect (trust on first use)",
        },
        sshConfig: {
          type: "string",
          description: "ssh_config file (default ~/.ssh/config)",
        },
      },
      required: [],
      additionalProperties: false,
    },
    options: {
      namespace: "cadmus.kobo",
      permission: "cadmus.kobo.deploy",
    },
    execute: async (input: unknown, context: ToolContext) => {
      const parsed = deployInputSchema.parse(input);
      await context.progress({
        status: `deploying to ${parsed.ip ?? "the Kobo"}`,
      });
      const text = await deploy(parsed, root, context.signal);
      return { content: text };
    },
  };
}
