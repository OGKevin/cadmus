import { execSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const outputDir = join(__dirname, "..", "generated");
const outputFile = join(outputDir, "version.ts");

let version;
try {
  version = execSync("git describe --tags --abbrev=0", {
    encoding: "utf-8",
  }).trim();
} catch {
  version = null;
}

mkdirSync(outputDir, { recursive: true });
writeFileSync(
  outputFile,
  `export const LATEST_VERSION = ${JSON.stringify(version)};\n`,
);

console.log(`Generated version: ${version ?? "null"}`);
