import type { ToolContext } from "@opencode/plugin/promise/tool";
import {
  report,
  restartArgs,
  restartInputSchema,
  runInShell,
  type RestartInput,
} from "./common.ts";

async function restart(
  input: RestartInput,
  root: string,
  signal: AbortSignal,
): Promise<string> {
  const result = await runInShell(root, ["cadmuscli", ...restartArgs(input)], {
    signal,
  });
  if (result.code === 0) return result.output.trim();
  return report("restart", result);
}

export function createRestartTool(root: string) {
  return {
    name: "restart",
    description: [
      "Restart Cadmus on a Kobo over SSH.",
      "Sends SIGUSR1, which makes Cadmus write /tmp/restart and lets cadmus.sh relaunch it.",
      "Only signals a process whose cwd is the cadmus-tst install.",
      "Use this after a deploy to put the new build on screen.",
    ].join(" "),
    input: {
      type: "object",
      properties: {
        ip: {
          type: "string",
          description:
            "IP address or ~/.ssh/config alias of the Kobo, for example 192.168.1.128. Defaults to the KOBO_IP environment variable.",
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
            "Private key file, tried before ssh_config IdentityFile and the usual ~/.ssh keys",
        },
        identitiesOnly: {
          type: "boolean",
          description:
            "Only use configured identity files and matching ssh-agent keys; never offer unrelated agent identities (IdentitiesOnly in ssh_config is not read)",
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
      permission: "cadmus.kobo.restart",
    },
    execute: async (input: unknown, context: ToolContext) => {
      const text = await restart(
        restartInputSchema.parse(input),
        root,
        context.signal,
      );
      return { content: text };
    },
  };
}
