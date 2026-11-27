import assert from "node:assert/strict";
import { test } from "node:test";

import type { LogFilter, LogPage } from "./companion.ts";
import {
  anchorScrollTop,
  appendPage,
  backwardPage,
  copyText,
  describeLogLine,
  estimateLogRowHeight,
  filterTargetTree,
  forwardPage,
  formatBytes,
  formatSnapshotTime,
  highlightJson,
  LOG_PAGE_LIMIT,
  MAX_LOADED_LINES,
  newerCursor,
  offsetRange,
  olderCursor,
  prependPage,
  sameFilter,
  startWindow,
  tailOffset,
  targetTree,
  windowLines,
} from "./companion-logs.ts";

function page(overrides: Partial<LogPage>): LogPage {
  return {
    lines: [],
    start: 0,
    end: 0,
    scannedTo: 0,
    size: 0,
    partial: false,
    ...overrides,
  };
}

const line = { offset: 10, text: "row", cutFrom: 0 };

test("backward complete page with lines continues from its start", () => {
  const entry = backwardPage(
    page({ lines: [line], start: 10, end: 40, scannedTo: 10, size: 100 }),
  );
  assert.equal(entry.olderCursor, 10);
  assert.equal(entry.newerCursor, 40);
});

test("backward page reaching file start has no older cursor", () => {
  const entry = backwardPage(
    page({ lines: [line], start: 0, end: 40, scannedTo: 0, size: 100 }),
  );
  assert.equal(entry.olderCursor, null);
});

test("backward partial page continues from scanned position", () => {
  const entry = backwardPage(
    page({
      lines: [line],
      start: 150,
      end: 200,
      scannedTo: 60,
      size: 300,
      partial: true,
    }),
  );
  assert.equal(entry.olderCursor, 60);
  assert.equal(entry.newerCursor, 200);
});

test("backward empty complete page reached the start", () => {
  const entry = backwardPage(page({ start: 50, end: 50, size: 100 }));
  assert.equal(entry.olderCursor, null);
});

test("backward page at end of file has no newer cursor", () => {
  const entry = backwardPage(
    page({ lines: [line], start: 10, end: 100, scannedTo: 10, size: 100 }),
  );
  assert.equal(entry.newerCursor, null);
});

test("forward complete page continues from its end", () => {
  const entry = forwardPage(
    page({ lines: [line], start: 0, end: 50, scannedTo: 50, size: 100 }),
  );
  assert.equal(entry.newerCursor, 50);
  assert.equal(entry.olderCursor, null);
});

test("forward partial page continues from scanned position", () => {
  const entry = forwardPage(
    page({
      lines: [line],
      start: 20,
      end: 40,
      scannedTo: 60,
      size: 100,
      partial: true,
    }),
  );
  assert.equal(entry.newerCursor, 60);
  assert.equal(entry.olderCursor, 20);
});

test("forward empty complete page reaches the end", () => {
  const entry = forwardPage(page({ start: 100, end: 100, size: 100 }));
  assert.equal(entry.newerCursor, null);
});

test("window cursors come from its edge pages", () => {
  const first = backwardPage(
    page({ lines: [line], start: 100, end: 200, scannedTo: 100, size: 300 }),
  );
  const last = forwardPage(
    page({ lines: [line], start: 200, end: 250, scannedTo: 250, size: 300 }),
  );
  const window = appendPage(startWindow(first), last);
  assert.equal(olderCursor(window), 100);
  assert.equal(newerCursor(window), 250);
  assert.equal(tailOffset(window), 250);
});

test("prepend keeps at most the loaded line budget", () => {
  const rows = (base: number, count: number) =>
    Array.from({ length: count }, (_, index) => ({
      offset: base + index,
      text: `${base + index}`,
      cutFrom: 0,
    }));
  let window = startWindow(
    backwardPage(
      page({
        lines: rows(0, LOG_PAGE_LIMIT),
        start: 0,
        end: LOG_PAGE_LIMIT,
        scannedTo: 0,
        size: 1_000_000,
      }),
    ),
  );
  for (let i = 1; i <= 20; i += 1) {
    window = prependPage(
      window,
      backwardPage(
        page({
          lines: rows(i * LOG_PAGE_LIMIT, LOG_PAGE_LIMIT),
          start: i * LOG_PAGE_LIMIT,
          end: (i + 1) * LOG_PAGE_LIMIT,
          scannedTo: i * LOG_PAGE_LIMIT,
          size: 1_000_000,
        }),
      ),
    );
  }
  assert.equal(window.pages.length, 8);
  assert.equal(windowLines(window).length, MAX_LOADED_LINES);
  assert.equal(windowLines(window)[0]?.offset, 20 * LOG_PAGE_LIMIT);
});

test("append keeps at most the loaded line budget", () => {
  const rows = (base: number, count: number) =>
    Array.from({ length: count }, (_, index) => ({
      offset: base + index,
      text: `${base + index}`,
      cutFrom: 0,
    }));
  let window = startWindow(
    forwardPage(
      page({
        lines: rows(0, LOG_PAGE_LIMIT),
        start: 0,
        end: LOG_PAGE_LIMIT,
        scannedTo: LOG_PAGE_LIMIT,
        size: 1_000_000,
      }),
    ),
  );
  for (let i = 1; i <= 20; i += 1) {
    window = appendPage(
      window,
      forwardPage(
        page({
          lines: rows(i * LOG_PAGE_LIMIT, LOG_PAGE_LIMIT),
          start: i * LOG_PAGE_LIMIT,
          end: (i + 1) * LOG_PAGE_LIMIT,
          scannedTo: (i + 1) * LOG_PAGE_LIMIT,
          size: 1_000_000,
        }),
      ),
    );
  }
  assert.equal(window.pages.length, 8);
  assert.equal(windowLines(window).length, MAX_LOADED_LINES);
  assert.equal(windowLines(window).at(-1)?.offset, 21 * LOG_PAGE_LIMIT - 1);
});

test("empty partial pages only move the older cursor", () => {
  let window = startWindow(
    backwardPage(
      page({
        lines: [{ offset: 500, text: "row", cutFrom: 0 }],
        start: 500,
        end: 520,
        scannedTo: 500,
        size: 2000,
      }),
    ),
  );
  for (let i = 1; i <= 40; i += 1) {
    window = prependPage(
      window,
      backwardPage(
        page({
          start: 500 - i * 10,
          end: 510 - i * 10,
          scannedTo: 500 - i * 10,
          size: 2000,
          partial: true,
        }),
      ),
    );
  }
  assert.equal(window.pages.length, 1);
  assert.equal(olderCursor(window), 100);
  assert.equal(newerCursor(window), 520);
  assert.deepEqual(
    windowLines(window).map((row) => row.text),
    ["row"],
  );
});

test("empty partial pages only move the newer cursor", () => {
  let window = startWindow(
    forwardPage(
      page({
        lines: [{ offset: 0, text: "row", cutFrom: 0 }],
        start: 0,
        end: 20,
        scannedTo: 20,
        size: 2000,
      }),
    ),
  );
  for (let i = 1; i <= 20; i += 1) {
    window = appendPage(
      window,
      forwardPage(
        page({
          start: 10 * i,
          end: 10 * i + 10,
          scannedTo: 10 * i + 10,
          size: 2000,
          partial: true,
        }),
      ),
    );
  }
  assert.equal(window.pages.length, 1);
  assert.equal(newerCursor(window), 210);
  assert.equal(olderCursor(window), null);
  assert.deepEqual(
    windowLines(window).map((row) => row.text),
    ["row"],
  );
});

test("window lines flatten pages in file order", () => {
  const first = backwardPage(
    page({
      lines: [{ offset: 0, text: "a", cutFrom: 0 }],
      start: 0,
      end: 10,
      size: 30,
    }),
  );
  const second = forwardPage(
    page({
      lines: [{ offset: 10, text: "b", cutFrom: 0 }],
      start: 10,
      end: 20,
      size: 30,
    }),
  );
  const window = appendPage(startWindow(first), second);
  assert.deepEqual(
    windowLines(window).map((row) => row.text),
    ["a", "b"],
  );
});

test("describe JSON log line with nested message", () => {
  const row = describeLogLine(
    JSON.stringify({
      timestamp: "2026-01-02T03:04:05Z",
      level: "WARN",
      target: "cadmus::app",
      fields: { message: "battery low" },
    }),
  );
  assert.ok(row);
  assert.equal(row.level, "WARN");
  assert.equal(row.target, "cadmus::app");
  assert.equal(row.message, "battery low");
  assert.match(row.pretty, /"battery low"/);
});

test("describe JSON log line with top-level message", () => {
  const row = describeLogLine(JSON.stringify({ message: "hello" }));
  assert.equal(row?.message, "hello");
  assert.equal(row?.level, "");
});

test("non-JSON and non-object lines are plain text", () => {
  assert.equal(describeLogLine("INFO plain text"), null);
  assert.equal(describeLogLine("[1,2,3]"), null);
  assert.equal(describeLogLine("42"), null);
});

test("highlightJson joins back to the input and marks each kind", () => {
  const text = JSON.stringify(
    {
      timestamp: "2026-01-02T03:04:05Z",
      level: "TRACE",
      fields: { message: 'say "hi"', count: 3, ok: true, none: null },
      target: "a::b",
    },
    null,
    2,
  );
  const tokens = highlightJson(text);
  assert.equal(tokens.map((token) => token.text).join(""), text);
  assert.deepEqual(
    tokens.filter((token) => token.kind === "key").map((token) => token.text),
    [
      '"timestamp"',
      '"level"',
      '"fields"',
      '"message"',
      '"count"',
      '"ok"',
      '"none"',
      '"target"',
    ],
  );
  assert.equal(
    tokens
      .filter((token) => token.kind === "string")
      .map((token) => token.text)
      .includes('"TRACE"'),
    true,
  );
  assert.deepEqual(
    tokens
      .filter((token) => token.kind === "number")
      .map((token) => token.text),
    ["3"],
  );
  assert.deepEqual(
    tokens
      .filter((token) => token.kind === "literal")
      .map((token) => token.text),
    ["true", "null"],
  );
  assert.equal(
    tokens
      .filter((token) => token.kind === "punctuation")
      .map((token) => token.text)
      .join("")
      .includes("{"),
    true,
  );
  assert.equal(
    tokens.find(
      (token) => token.kind === "string" && token.text.includes("say"),
    )?.text,
    JSON.stringify('say "hi"'),
  );
});

test("highlightJson keeps escaped quotes and backslashes inside strings", () => {
  const value = 'back\\slash and "quotes"';
  const text = JSON.stringify({ message: value }, null, 2);
  const tokens = highlightJson(text);
  assert.equal(tokens.map((token) => token.text).join(""), text);
  assert.equal(
    tokens.find((token) => token.kind === "string")?.text,
    JSON.stringify(value),
  );
});

test("highlightJson leaves plain text as one token", () => {
  assert.deepEqual(highlightJson("info log line"), [
    { kind: "plain", text: "info log line" },
  ]);
});

test("formatSnapshotTime pads hours, minutes and seconds", () => {
  assert.equal(formatSnapshotTime(new Date(2026, 10, 25, 4, 5, 9)), "04:05:09");
  assert.equal(
    formatSnapshotTime(new Date(2026, 10, 25, 23, 59, 0)),
    "23:59:00",
  );
});

test("formatBytes uses binary units", () => {
  assert.equal(formatBytes(512), "512 B");
  assert.equal(formatBytes(2048), "2.0 KB");
  assert.equal(formatBytes(20 * 1024 * 1024), "20 MB");
});

test("sameFilter compares levels and targets in any order", () => {
  const base: LogFilter = {
    levels: ["warn", "info"],
    targets: ["a", "b"],
    text: "",
  };
  assert.equal(sameFilter(base, { ...base, levels: [...base.levels] }), true);
  assert.equal(
    sameFilter(base, {
      ...base,
      levels: [...base.levels].reverse(),
      targets: [...base.targets].reverse(),
    }),
    true,
  );
  assert.equal(sameFilter(base, { ...base, levels: ["warn"] }), false);
  assert.equal(sameFilter(base, { ...base, targets: ["a"] }), false);
  assert.equal(sameFilter(base, { ...base, targets: ["a", "c"] }), false);
  assert.equal(sameFilter(base, { ...base, text: "x" }), false);
});

test("targetTree lists a target above its children in path order", () => {
  const nodes = targetTree([
    { target: "cadmus_core::device::rtc", lines: 5 },
    { target: "cadmus_core::device::wifi", lines: 3 },
    { target: "cadmus_core::lease", lines: 7 },
  ]);
  assert.deepEqual(
    nodes.map((node) => node.value),
    [
      "cadmus_core",
      "cadmus_core::device",
      "cadmus_core::device::rtc",
      "cadmus_core::device::wifi",
      "cadmus_core::lease",
    ],
  );
});

test("targetTree gives a prefix the lines of everything under it", () => {
  const nodes = targetTree([
    { target: "cadmus_core::library::importer", lines: 40 },
    { target: "cadmus_core::library::db", lines: 10 },
    { target: "cadmus_core::device::rtc", lines: 5 },
    { target: "cadmus_core::device::wifi", lines: 2 },
    { target: "cadmus::app", lines: 25 },
  ]);
  const byValue = new Map(nodes.map((node) => [node.value, node.lines]));
  assert.equal(byValue.get("cadmus_core"), 57);
  assert.equal(byValue.get("cadmus_core::library"), 50);
  assert.equal(byValue.get("cadmus_core::device"), 7);
  assert.equal(byValue.get("cadmus_core::device::rtc"), 5);
});

test("targetTree collapses a prefix whose only child carries its count", () => {
  const nodes = targetTree([
    { target: "cadmus::app", lines: 4 },
    { target: "cadmus_core", lines: 2 },
  ]);
  assert.deepEqual(
    nodes.map((node) => node.value),
    ["cadmus::app", "cadmus_core"],
  );
});

test("targetTree keeps a prefix that has lines of its own", () => {
  const nodes = targetTree([
    { target: "cadmus_core", lines: 2 },
    { target: "cadmus_core::db", lines: 3 },
  ]);
  assert.deepEqual(
    nodes.map((node) => [node.value, node.lines]),
    [
      ["cadmus_core", 5],
      ["cadmus_core::db", 3],
    ],
  );
});

test("targetTree sorts a target directly above its sub-targets", () => {
  const nodes = targetTree([
    { target: "a1", lines: 1 },
    { target: "a::b", lines: 2 },
    { target: "a", lines: 3 },
  ]);
  assert.deepEqual(
    nodes.map((node) => node.value),
    ["a", "a::b", "a1"],
  );
});

test("targetTree counts a target's own lines and its :: sub-targets only", () => {
  const nodes = targetTree([
    { target: "a::b", lines: 5 },
    { target: "a::bc", lines: 7 },
    { target: "a", lines: 1 },
  ]);
  const byValue = new Map(nodes.map((node) => [node.value, node.lines]));
  assert.equal(byValue.get("a"), 13);
  assert.equal(byValue.get("a::b"), 5);
  assert.equal(byValue.get("a::bc"), 7);
  assert.equal(byValue.has("a::b::c"), false);
});

test("filterTargetTree keeps the ancestors of a match", () => {
  const nodes = targetTree([
    { target: "cadmus_core::device::rtc", lines: 5 },
    { target: "cadmus_core::device::wifi", lines: 3 },
    { target: "cadmus_core::lease", lines: 7 },
  ]);
  assert.deepEqual(
    filterTargetTree(nodes, "WIFI").map((node) => node.value),
    ["cadmus_core", "cadmus_core::device", "cadmus_core::device::wifi"],
  );
  assert.deepEqual(
    filterTargetTree(nodes, "Lease").map((node) => node.value),
    ["cadmus_core", "cadmus_core::lease"],
  );
  assert.deepEqual(
    filterTargetTree(nodes, "").map((node) => node.value),
    nodes.map((node) => node.value),
  );
  assert.deepEqual(filterTargetTree(nodes, "zzz"), []);
});

test("copyText reports failure without a document to select from", async () => {
  assert.equal(await copyText("payload"), false);
});

test("offsetRange spans the loaded lines between both ends", () => {
  const lines = [
    { offset: 10 },
    { offset: 40 },
    { offset: 90 },
    { offset: 100 },
  ];
  assert.deepEqual(offsetRange(lines, 10, 90), [10, 40, 90]);
  assert.deepEqual(offsetRange(lines, 90, 10), [10, 40, 90]);
  assert.deepEqual(offsetRange(lines, 40, 40), [40]);
  assert.deepEqual(offsetRange(lines, 10, 100), [10, 40, 90, 100]);
});

test("offsetRange yields nothing for a line outside the window", () => {
  const lines = [{ offset: 10 }, { offset: 40 }];
  assert.deepEqual(offsetRange(lines, 10, 77), []);
  assert.deepEqual(offsetRange(lines, 77, 40), []);
});

test("estimateLogRowHeight grows with long text and shrinks with width", () => {
  const short = { offset: 0, text: "row", cutFrom: 0 };
  const long = {
    offset: 0,
    text: `{"timestamp":"2026-11-27T00:00:00Z","level":"INFO","fields":{"message":"${"x".repeat(300)}"},"target":"cadmus::core"}`,
    cutFrom: 0,
  };
  assert.equal(estimateLogRowHeight(short, 390), 33);
  assert.ok(estimateLogRowHeight(long, 390) > 49);
  assert.ok(estimateLogRowHeight(long, 1280) < estimateLogRowHeight(long, 390));
});

test("anchorScrollTop keeps a row above the viewport above it", () => {
  assert.equal(anchorScrollTop(100, -20), 120);
  assert.equal(anchorScrollTop(100, 0), 100);
  assert.equal(anchorScrollTop(0, -20), 20);
  assert.equal(anchorScrollTop(10, 20), 0);
});
