import { spawn } from "node:child_process";
import { existsSync, readdirSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { z } from "../../node_modules/zod/index.js";

export const INSTALL_DIR = "/mnt/onboard/.adds/cadmus-tst";
export const FEATURES = "test,kobo";

const HOST_PATTERN = /^[A-Za-z0-9._:%-]+$/;
const FEATURE_PATTERN = /^[A-Za-z0-9_-]+$/;

export const hostSchema = z.string().regex(HOST_PATTERN, "invalid host");

export const deployInputSchema = z
  .object({
    ip: hostSchema,
    features: z.string().optional(),
  })
  .strict();

export const restartInputSchema = z.object({ ip: hostSchema }).strict();

export type DeployInput = z.infer<typeof deployInputSchema>;
export type RestartInput = z.infer<typeof restartInputSchema>;

/**
 * Cargo features for a cadmus-tst binary.
 *
 * `test,kobo` is always first so the install and data paths stay on the test
 * tree. Extra names are appended; repeats of the required pair are dropped.
 */
export function cargoFeatures(extra?: string): string {
  const required = FEATURES.split(",");
  const extras = (extra ?? "")
    .split(",")
    .map((feature) => feature.trim())
    .filter((feature) => feature.length > 0);
  for (const feature of extras) {
    if (!FEATURE_PATTERN.test(feature)) {
      throw new Error(`invalid feature: ${feature}`);
    }
  }
  const seen = new Set(required);
  const features = [...required];
  for (const feature of extras) {
    if (seen.has(feature)) continue;
    seen.add(feature);
    features.push(feature);
  }
  return features.join(",");
}

const SSH_BATCH = "ssh -o BatchMode=yes -o ConnectTimeout=15";

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
 * Rsync arguments for the test install.
 *
 * IPv6 hosts are bracketed because rsync splits host and path on a colon.
 */
export function rsyncArgs(ip: string, files: string[]) {
  return [
    "-rc",
    "-e",
    SSH_BATCH,
    ...files,
    `root@${rsyncHost(ip)}:${INSTALL_DIR}`,
  ];
}

function rsyncHost(ip: string) {
  return ip.includes(":") ? `[${ip}]` : ip;
}

export function resolveBin(name: string) {
  const dirs = (process.env.PATH ?? "").split(":").concat(FALLBACK_BIN_DIRS);
  for (const dir of dirs) {
    if (!dir) continue;
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

export function distEntries(root: string) {
  return readdirSync(join(root, "dist"), { withFileTypes: true })
    .filter((entry) => !entry.name.startsWith("."))
    .map((entry) => join("dist", entry.name));
}

export function report(name: string, result: RunResult) {
  const status = result.code === 0 ? "ok" : `failed (exit ${result.code})`;
  if (result.code === 0) return `${name}: ${status}`;
  const body = result.output.trim();
  return body ? `${name}: ${status}\n${body}` : `${name}: ${status}`;
}
