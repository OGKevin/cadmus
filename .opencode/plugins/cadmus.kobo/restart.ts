import type { ToolContext } from "@opencode/plugin/promise/tool";
import { assertHost, resolveBin, run } from "./common.ts";

type RestartInput = {
  ip: string;
};

/// Exit codes the remote script uses.
const EXIT_NOT_RUNNING = 1;
const EXIT_NOT_RESTARTED = 2;

export const RESTART_SCRIPT = `BEFORE=$(pidof cadmus 2>/dev/null | tr ' ' '\\n' | head -1)
if [ -z "$BEFORE" ]; then
  echo "cadmus is not running"
  exit ${EXIT_NOT_RUNNING}
fi

kill -USR1 "$BEFORE"
sleep 12

AFTER=$(pidof cadmus 2>/dev/null | tr ' ' '\\n' | head -1)
echo "cadmus pid: $BEFORE -> \${AFTER:-none}"
if [ -n "$AFTER" ] && [ "$AFTER" != "$BEFORE" ]; then
  echo "RESULT: restarted"
  exit 0
fi
echo "RESULT: cadmus did not restart"
exit ${EXIT_NOT_RESTARTED}
`;

async function restart(input: RestartInput, signal: AbortSignal) {
  assertHost(input.ip);
  const result = await run(
    resolveBin("ssh"),
    [
      "-o",
      "BatchMode=yes",
      "-o",
      "ConnectTimeout=15",
      `root@${input.ip}`,
      "sh",
      "-s",
    ],
    { signal, stdin: RESTART_SCRIPT },
  );
  const body = result.output.trim();
  if (result.code === EXIT_NOT_RUNNING) {
    return `Cadmus is not running on ${input.ip}. Start it from the device, then try again.`;
  }
  if (result.code === EXIT_NOT_RESTARTED) {
    return `Cadmus did not restart on ${input.ip}:\n${body}`;
  }
  if (result.code !== 0) {
    return `ssh to ${input.ip} failed (exit ${result.code})\n${body}\nThe device may be asleep; wake it or reboot it and try again.`;
  }
  return `Restart on ${input.ip}:\n${body}`;
}

export const restartTool = {
  name: "restart",
  description: [
    "Restart Cadmus on a Kobo over SSH.",
    "Sends SIGUSR1, which makes Cadmus write /tmp/restart and lets cadmus.sh relaunch it.",
    "Use this after a deploy to put the new build on screen.",
  ].join(" "),
  input: {
    type: "object",
    properties: {
      ip: {
        type: "string",
        description: "IP address of the Kobo, for example 192.168.1.128",
      },
    },
    required: ["ip"],
    additionalProperties: false,
  },
  options: {
    namespace: "cadmus.kobo",
    permission: "cadmus.kobo.restart",
  },
  execute: async (input: unknown, context: ToolContext) => {
    const text = await restart(input as RestartInput, context.signal);
    return { content: text };
  },
};
