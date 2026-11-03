#!/usr/bin/env node
// Repairs an interrupted chrome-headless-shell download before installing it.
//
// @puppeteer/browsers extracts archives in place into
// <cache>/chrome-headless-shell/<platform>-<buildId> and refuses to reuse a
// version directory whose executable is missing, so a killed download poisons
// the cache and fails every later install.
//
// A version directory is kept only once it is known to be finished: this
// script writes a completion marker after a successful install, and a
// directory that predates the marker is adopted only after it has stopped
// changing. Repair runs under an exclusive lock and never removes a directory
// that was modified recently, because another installer sharing the cache may
// still be extracting into it.

import { spawnSync } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const BROWSER = "chrome-headless-shell";
const COMPLETE_MARKER = ".cadmus-install-complete";
const CACHE_DIR =
  process.env.PUPPETEER_CACHE_DIR ?? join(homedir(), ".cache", "puppeteer");
const BROWSER_DIR = join(CACHE_DIR, BROWSER);
const LOCK_PATH = join(CACHE_DIR, ".cadmus-install.lock");
const LOCK_STALE_MS = 15 * 60_000;
const SETTLE_MS = 60_000;

// The layout is <version-dir>/<platform-dir>/chrome-headless-shell today; walk
// a little deeper so changed nesting never makes us delete a valid install.
function hasExecutable(dir, depth = 0) {
  if (depth > 3) return false;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (hasExecutable(join(dir, entry.name), depth + 1)) return true;
    } else if (entry.isFile() && entry.name === BROWSER) {
      return true;
    }
  }
  return false;
}

// Extraction writes files continuously, so a version directory that has been
// idle for a while is neither being filled by this run nor by another
// installer; both the completeness and the removal decisions depend on it.
function newestMtimeMs(dir, depth = 0) {
  let newest = 0;
  try {
    newest = statSync(dir).mtimeMs;
  } catch {
    return newest;
  }
  if (depth > 3) return newest;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    newest = Math.max(newest, newestMtimeMs(join(dir, entry.name), depth + 1));
  }
  return newest;
}

function isQuiescent(dir) {
  return Date.now() - newestMtimeMs(dir) > SETTLE_MS;
}

function acquireLock() {
  mkdirSync(CACHE_DIR, { recursive: true });
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const fd = openSync(LOCK_PATH, "wx");
      writeFileSync(fd, `${process.pid}\n`);
      closeSync(fd);
      return true;
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
      let age;
      try {
        age = Date.now() - statSync(LOCK_PATH).mtimeMs;
      } catch {
        continue;
      }
      if (age < LOCK_STALE_MS) return false;
      rmSync(LOCK_PATH, { force: true });
    }
  }
  return false;
}

function releaseLock() {
  try {
    rmSync(LOCK_PATH, { force: true });
  } catch {
    // A leftover lock is taken over once it goes stale.
  }
}

function markComplete(versionDir) {
  writeFileSync(join(versionDir, COMPLETE_MARKER), `${Date.now()}\n`);
}

function isComplete(versionDir) {
  return (
    existsSync(join(versionDir, COMPLETE_MARKER)) && hasExecutable(versionDir)
  );
}

function versionDirs() {
  if (!existsSync(BROWSER_DIR)) return [];
  return readdirSync(BROWSER_DIR, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => join(BROWSER_DIR, entry.name));
}

function removeIncompleteInstalls() {
  for (const versionDir of versionDirs()) {
    try {
      if (hasExecutable(versionDir)) {
        if (!isComplete(versionDir) && isQuiescent(versionDir)) {
          markComplete(versionDir);
        }
        continue;
      }
      if (!isQuiescent(versionDir)) continue;
      rmSync(versionDir, { recursive: true, force: true });
      console.log(`Removed incomplete ${BROWSER} install: ${versionDir}`);
    } catch (error) {
      console.warn(`Could not check ${versionDir}: ${error.message}`);
    }
  }
}

// Only directories this run produced are marked: a directory that already
// existed belongs to another installer and may still be filling up.
function markInstalled(previous) {
  for (const versionDir of versionDirs()) {
    if (previous.has(versionDir)) continue;
    try {
      if (hasExecutable(versionDir)) markComplete(versionDir);
    } catch (error) {
      console.warn(`Could not mark ${versionDir}: ${error.message}`);
    }
  }
}

const lockHeld = acquireLock();
try {
  if (lockHeld) {
    removeIncompleteInstalls();
  } else {
    console.warn(`Another installer holds ${LOCK_PATH}; skipping repair`);
  }
  const previous = new Set(versionDirs());

  const { status, error } = spawnSync(
    "puppeteer",
    ["browsers", "install", BROWSER],
    { stdio: "inherit" },
  );
  if (error) {
    console.error(`Could not run puppeteer: ${error.message}`);
    process.exitCode = 1;
  } else {
    if (status === 0) markInstalled(previous);
    process.exitCode = status ?? 1;
  }
} finally {
  if (lockHeld) releaseLock();
}
