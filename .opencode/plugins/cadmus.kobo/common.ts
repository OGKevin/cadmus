import { spawn } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { z } from "../../node_modules/zod/index.js";

const HOST_PATTERN = /^[A-Za-z0-9._:%-]+$/;

export const hostSchema = z.string().regex(HOST_PATTERN, "invalid host");

const sshOptionsShape = {
  port: z.number().int().min(1).max(65535).optional(),
  user: z.string().min(1).optional(),
  identity: z.string().min(1).optional(),
  identitiesOnly: z.boolean().optional(),
  knownHosts: z.string().min(1).optional(),
  acceptNew: z.boolean().optional(),
  sshConfig: z.string().min(1).optional(),
};

export const deployInputSchema = z
  .object({
    ip: hostSchema.optional(),
    features: z.string().optional(),
    restart: z.boolean().optional(),
    ...sshOptionsShape,
  })
  .strict();

export const restartInputSchema = z
  .object({
    ip: hostSchema.optional(),
    ...sshOptionsShape,
  })
  .strict();

export type DeployInput = z.infer<typeof deployInputSchema>;
export type RestartInput = z.infer<typeof restartInputSchema>;

type SshInput = {
  port?: number;
  user?: string;
  identity?: string;
  identitiesOnly?: boolean;
  knownHosts?: string;
  acceptNew?: boolean;
  sshConfig?: string;
};

const FALLBACK_BIN_DIRS = [
  "/run/current-system/sw/bin",
  join(homedir(), ".nix-profile", "bin"),
  ...(process.env.DEVENV_STATE ? [join(process.env.DEVENV_STATE, "bin")] : []),
  "/usr/local/bin",
  "/usr/bin",
  "/bin",
];

export type RunResult = { code: number; output: string };

/**
 * SSH flags forwarded to `cadmuscli kobo` for both subcommands.
 *
 * The CLI applies them on top of `~/.ssh/config` and its own defaults, so the
 * plugin never reads an ssh config itself.
 */
export function sshFlags(input: SshInput): string[] {
  const flags: string[] = [];
  if (input.port !== undefined) flags.push("--port", String(input.port));
  if (input.user !== undefined) flags.push("--user", input.user);
  if (input.identity !== undefined) flags.push("--identity", input.identity);
  if (input.identitiesOnly === true) flags.push("--identities-only");
  if (input.knownHosts !== undefined) {
    flags.push("--known-hosts", input.knownHosts);
  }
  if (input.acceptNew === true) flags.push("--accept-new");
  if (input.sshConfig !== undefined)
    flags.push("--ssh-config", input.sshConfig);
  return flags;
}

/** Arguments for `cadmuscli kobo deploy`. */
export function deployArgs(input: DeployInput): string[] {
  const args = ["kobo", "deploy"];
  if (input.ip !== undefined) args.push("--ip", input.ip);
  if (input.features !== undefined) args.push("--features", input.features);
  if (input.restart === true) args.push("--restart");
  args.push(...sshFlags(input));
  return args;
}

/** Arguments for `cadmuscli kobo restart`. */
export function restartArgs(input: RestartInput): string[] {
  const args = ["kobo", "restart"];
  if (input.ip !== undefined) args.push("--ip", input.ip);
  args.push(...sshFlags(input));
  return args;
}

function pathDirs() {
  const inherited = (process.env.PATH ?? "").split(":").filter(Boolean);
  return [...new Set([...inherited, ...FALLBACK_BIN_DIRS])];
}

export function resolveBin(name: string) {
  for (const dir of pathDirs()) {
    const candidate = join(dir, name);
    if (existsSync(candidate)) return candidate;
  }
  return name;
}

export function run(
  command: string,
  args: string[],
  options: { cwd?: string; signal?: AbortSignal } = {},
): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    if (options.signal?.aborted) {
      reject(
        options.signal.reason ??
          new DOMException("This operation was aborted", "AbortError"),
      );
      return;
    }
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, PATH: pathDirs().join(":") },
      signal: options.signal,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => (output += chunk));
    child.stderr.on("data", (chunk) => (output += chunk));
    child.on("error", reject);
    child.on("close", (code) => resolve({ code: code ?? -1, output }));
  });
}

function samePath(left: string, right: string) {
  try {
    return realpathSync(left) === realpathSync(right);
  } catch {
    return false;
  }
}

function insideDevenv(root: string) {
  const devRoot = process.env.DEVENV_ROOT;
  if (!devRoot) return false;
  return samePath(devRoot, root);
}

export async function runInShell(
  root: string,
  args: string[],
  options: { signal?: AbortSignal } = {},
): Promise<RunResult> {
  const [command, ...rest] = args;
  if (!command) throw new Error("no command to run");
  if (insideDevenv(root)) {
    const resolved = resolveBin(command);
    if (resolved !== command) {
      return run(resolved, rest, { cwd: root, ...options });
    }
  }
  return run(resolveBin("devenv"), ["shell", "--", ...args], {
    cwd: root,
    ...options,
  });
}

export function report(name: string, result: RunResult) {
  const status = result.code === 0 ? "ok" : `failed (exit ${result.code})`;
  if (result.code === 0) return `${name}: ${status}`;
  const body = result.output.trim();
  return body ? `${name}: ${status}\n${body}` : `${name}: ${status}`;
}
