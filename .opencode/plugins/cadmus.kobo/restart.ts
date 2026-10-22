import type { ToolContext } from "@opencode/plugin/promise/tool";
import {
  INSTALL_DIR,
  resolveBin,
  restartInputSchema,
  run,
  type RestartInput,
} from "./common.ts";

const EXIT_NOT_RUNNING = 1;
const EXIT_NOT_RESTARTED = 2;

/**
 * Remote shell that restarts only the Cadmus process whose cwd is the test
 * install. Polls pidof once a second for up to 30 s.
 */
export const RESTART_SCRIPT = `cadmus_pid() {
  for pid in $(pidof cadmus 2>/dev/null); do
    if [ "$(readlink /proc/$pid/cwd 2>/dev/null)" = "${INSTALL_DIR}" ]; then
      echo "$pid"
      return 0
    fi
  done
  return 1
}

BEFORE=$(cadmus_pid) || BEFORE=
if [ -z "$BEFORE" ]; then
  echo "cadmus is not running"
  exit ${EXIT_NOT_RUNNING}
fi

env kill -USR1 "$BEFORE"

i=0
while [ "$i" -lt 30 ]; do
  sleep 1
  AFTER=$(cadmus_pid) || AFTER=
  if [ -n "$AFTER" ] && [ "$AFTER" != "$BEFORE" ]; then
    echo "cadmus pid: $BEFORE -> $AFTER"
    echo "RESULT: restarted"
    exit 0
  fi
  i=$((i + 1))
done

AFTER=$(cadmus_pid) || AFTER=
echo "cadmus pid: $BEFORE -> \${AFTER:-none}"
echo "RESULT: cadmus did not restart"
exit ${EXIT_NOT_RESTARTED}
`;

async function restart(input: RestartInput, signal: AbortSignal) {
  const result = await run(
    resolveBin("ssh"),
    [
      "-o",
      "BatchMode=yes",
      "-o",
      "ConnectTimeout=15",
      `root@${input.ip}`,
      RESTART_SCRIPT,
    ],
    { signal },
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
    "Only signals a process whose cwd is the cadmus-tst install.",
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
    const text = await restart(restartInputSchema.parse(input), context.signal);
    return { content: text };
  },
};
