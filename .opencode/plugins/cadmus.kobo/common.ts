import { spawn } from "node:child_process";
import { existsSync, readdirSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export const INSTALL_DIR = "/mnt/onboard/.adds/cadmus-tst";
export const DEFAULT_FEATURES = "test,kobo";

const HOST_PATTERN = /^[A-Za-z0-9._:%-]+$/;

const FALLBACK_BIN_DIRS = [
  "/run/current-system/sw/bin",
  join(homedir(), ".nix-profile", "bin"),
  ...(process.env.DEVENV_STATE ? [join(process.env.DEVENV_STATE, "bin")] : []),
  "/usr/local/bin",
  "/usr/bin",
  "/bin",
];

export type RunResult = { code: number; output: string };

export function assertHost(ip: string) {
  if (!HOST_PATTERN.test(ip)) {
    throw new Error(`invalid host: ${ip}`);
  }
}

export function buildArgs(features: string) {
  return ["build-kobo", "--features", features];
}

export function distArgs() {
  return ["dist"];
}

export function rsyncArgs(ip: string, files: string[]) {
  return ["-Prc", ...files, `root@${rsyncHost(ip)}:${INSTALL_DIR}`];
}

/// The host as rsync wants it.
///
/// Rsync separates the remote host from the path with a colon, so an IPv6
/// address has to be bracketed.
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
  options: { cwd?: string; signal?: AbortSignal; stdin?: string },
): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    if (options.signal?.aborted) {
      reject(new Error(`${command} was cancelled before it started`));
      return;
    }

    const child = spawn(command, args, {
      cwd: options.cwd ?? process.cwd(),
      env: process.env,
    });
    let output = "";
    let settled = false;
    const settle = (result: RunResult) => {
      if (settled) return;
      settled = true;
      resolve(result);
    };
    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    child.stdout.on("data", (chunk) => (output += chunk));
    child.stderr.on("data", (chunk) => (output += chunk));
    child.on("error", fail);
    child.on("close", (code) => settle({ code: code ?? -1, output }));

    if (options.stdin !== undefined) {
      child.stdin.on("error", (error) => {
        output += `\nstdin error: ${error.message}`;
      });
      child.stdin.write(options.stdin);
      child.stdin.end();
    }

    options.signal?.addEventListener("abort", () => child.kill("SIGTERM"), {
      once: true,
    });
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
  options: { signal?: AbortSignal; stdin?: string },
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
  const body = result.output.trim();
  return body ? `${name}: ${status}\n${body}` : `${name}: ${status}`;
}
