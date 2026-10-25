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

const SSH_OPTIONS = "-o BatchMode=yes -o ConnectTimeout=15";

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
 * Files are only ever added or overwritten. Deploy never deletes from the
 * device, so a reader's own fonts, dictionaries, or stylesheets stay put even
 * when the distribution stops shipping a file that shares their directory.
 *
 * Owner and group are never transferred: `/mnt/onboard` is a FAT filesystem,
 * where rsync's chown fails with `Operation not permitted`.
 *
 * IPv6 hosts are bracketed because rsync splits host and path on a colon.
 */
export function rsyncArgs(ip: string, files: string[]) {
  return [
    "-rc",
    "--no-owner",
    "--no-group",
    "-e",
    sshCommand(),
    ...files,
    `root@${rsyncHost(ip)}:${INSTALL_DIR}`,
  ];
}

/**
 * Remote shell rsync runs for the transfer.
 *
 * rsync resolves this with `execvp`, so a bare `ssh` needs ssh on the child
 * PATH. Resolving it here keeps the transfer independent of the PATH the host
 * hands the plugin.
 */
export function sshCommand() {
  return `${quoteForRsync(resolveBin("ssh"))} ${SSH_OPTIONS}`;
}

/**
 * Quotes a path for rsync's `-e` value.
 *
 * rsync splits that value on whitespace, honouring single or double quotes, so
 * an executable under a directory with a space in it needs quoting. rsync does
 * not honour backslash escapes, hence quotes.
 */
export function quoteForRsync(path: string) {
  if (!/[\s'"]/.test(path)) return path;
  if (!path.includes("'")) return `'${path}'`;
  if (!path.includes('"')) return `"${path}"`;
  throw new Error(`cannot quote an ssh path holding both quote kinds: ${path}`);
}

function rsyncHost(ip: string) {
  return ip.includes(":") ? `[${ip}]` : ip;
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
