import type { LogFilter, LogLevel, LogLine, LogPage } from "./companion";

export const LOG_LEVELS: LogLevel[] = [
  "trace",
  "debug",
  "info",
  "warn",
  "error",
];
export const LOG_PAGE_LIMIT = 200;
/**
 * Most lines the viewer keeps loaded. The budget counts lines, not pages: a
 * page that scanned without finding a match only moves the window's edge
 * cursor, and the lines around the edge the reader is moving toward stay.
 */
export const MAX_LOADED_LINES = 8 * LOG_PAGE_LIMIT;

export type LogWindowPage = {
  page: LogPage;
  olderCursor: number | null;
  newerCursor: number | null;
};

export type LogWindow = {
  pages: LogWindowPage[];
};

export function backwardPage(page: LogPage): LogWindowPage {
  const next = page.partial
    ? page.scannedTo
    : page.lines.length === 0
      ? 0
      : page.start;
  return {
    page,
    olderCursor: next === 0 ? null : next,
    newerCursor: page.end >= page.size ? null : page.end,
  };
}

export function forwardPage(page: LogPage): LogWindowPage {
  const next = page.partial
    ? page.scannedTo
    : page.lines.length === 0
      ? page.size
      : page.end;
  return {
    page,
    olderCursor: page.start === 0 ? null : page.start,
    newerCursor: next >= page.size ? null : next,
  };
}

export function startWindow(entry: LogWindowPage): LogWindow {
  return { pages: [entry] };
}

function totalLines(pages: LogWindowPage[]): number {
  let lines = 0;
  for (const entry of pages) lines += entry.page.lines.length;
  return lines;
}

function trimTail(pages: LogWindowPage[]): LogWindowPage[] {
  const next = [...pages];
  let lines = totalLines(next);
  while (next.length > 1 && lines > MAX_LOADED_LINES) {
    lines -= next.pop()?.page.lines.length ?? 0;
  }
  return next;
}

function trimHead(pages: LogWindowPage[]): LogWindowPage[] {
  const next = [...pages];
  let lines = totalLines(next);
  while (next.length > 1 && lines > MAX_LOADED_LINES) {
    lines -= next.shift()?.page.lines.length ?? 0;
  }
  return next;
}

/** An empty backward page adds only the cursor to scan on from. */
function mergeBackward(
  edge: LogWindowPage,
  entry: LogWindowPage,
): LogWindowPage {
  return {
    page: {
      ...edge.page,
      start: entry.page.start,
      size: entry.page.size,
    },
    olderCursor: entry.olderCursor,
    newerCursor: edge.newerCursor,
  };
}

/** An empty forward page adds only the cursor to scan on from. */
function mergeForward(
  edge: LogWindowPage,
  entry: LogWindowPage,
): LogWindowPage {
  return {
    page: {
      ...edge.page,
      end: entry.page.end,
      size: entry.page.size,
    },
    olderCursor: edge.olderCursor,
    newerCursor: entry.newerCursor,
  };
}

export function prependPage(
  window: LogWindow,
  entry: LogWindowPage,
): LogWindow {
  const [edge, ...rest] = window.pages;
  const pages =
    edge !== undefined && entry.page.lines.length === 0
      ? [mergeBackward(edge, entry), ...rest]
      : [entry, ...window.pages];
  return { pages: trimTail(pages) };
}

export function appendPage(window: LogWindow, entry: LogWindowPage): LogWindow {
  const last = window.pages.at(-1);
  const pages =
    last !== undefined && entry.page.lines.length === 0
      ? [...window.pages.slice(0, -1), mergeForward(last, entry)]
      : [...window.pages, entry];
  return { pages: trimHead(pages) };
}

export function olderCursor(window: LogWindow): number | null {
  return window.pages[0]?.olderCursor ?? null;
}

export function newerCursor(window: LogWindow): number | null {
  return window.pages.at(-1)?.newerCursor ?? null;
}

export function tailOffset(window: LogWindow): number {
  return window.pages.at(-1)?.page.end ?? 0;
}

export function windowLines(window: LogWindow): LogLine[] {
  return window.pages.flatMap((entry) => entry.page.lines);
}

/**
 * Whether two filters select the same lines, so a draft equal to the applied
 * filter does not reload the window.
 */
export function sameFilter(a: LogFilter, b: LogFilter): boolean {
  return (
    a.text === b.text &&
    a.levels.length === b.levels.length &&
    a.levels.every((level) => b.levels.includes(level)) &&
    a.targets.length === b.targets.length &&
    a.targets.every((target) => b.targets.includes(target))
  );
}

export type TargetCount = { target: string; lines: number };

/** One choice of the Target picker: a target or one of its module prefixes. */
export type TargetNode = {
  value: string;
  lines: number;
};

/**
 * The Target picker's tree, built from the exact targets of a log.
 *
 * A target is the module that wrote the line, and the device keeps a line
 * when its target equals a picked value or continues it with `::`, so every
 * entry also selects the lines of the targets below it. That is why the picker
 * offers each target together with its module prefixes: a prefix carries the
 * added-up count of its descendants, and path order puts it directly above
 * them.
 *
 * A prefix with a single child and no lines of its own adds nothing to the
 * tree, so it is left out.
 */
export function targetTree(targets: TargetCount[]): TargetNode[] {
  const totals = new Map<string, number>();
  const children = new Map<string, Set<string>>();
  for (const { target, lines } of targets) {
    if (!target) continue;
    const parts = target.split("::");
    for (let depth = 1; depth <= parts.length; depth += 1) {
      const value = parts.slice(0, depth).join("::");
      totals.set(value, (totals.get(value) ?? 0) + lines);
      if (depth > 1) {
        const parent = parts.slice(0, depth - 1).join("::");
        const siblings = children.get(parent) ?? new Set<string>();
        siblings.add(value);
        children.set(parent, siblings);
      }
    }
  }
  const nodes: TargetNode[] = [];
  for (const [value, lines] of totals) {
    const kids = children.get(value);
    if (kids?.size === 1 && lines === totals.get([...kids][0])) continue;
    nodes.push({ value, lines });
  }
  return nodes.sort((a, b) => compareTargetPaths(a.value, b.value));
}

/**
 * Path order with `::` below every other character, so a target sits directly
 * above its sub-targets: `a`, `a::b`, `a1`.
 */
function compareTargetPaths(a: string, b: string): number {
  const left = a.replaceAll("::", "\u0000");
  const right = b.replaceAll("::", "\u0000");
  return left < right ? -1 : left > right ? 1 : 0;
}

/**
 * The nodes whose path contains `query`, plus the ancestors that give them
 * context. A blank query keeps the whole tree.
 */
export function filterTargetTree(
  nodes: TargetNode[],
  query: string,
): TargetNode[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return nodes;
  const known = new Set(nodes.map((node) => node.value));
  const keep = new Set<string>();
  for (const node of nodes) {
    if (!node.value.toLowerCase().includes(needle)) continue;
    keep.add(node.value);
    let parent = node.value;
    while (parent.includes("::")) {
      parent = parent.slice(0, parent.lastIndexOf("::"));
      if (known.has(parent)) keep.add(parent);
    }
  }
  return nodes.filter((node) => keep.has(node.value));
}

/**
 * Copies `text` to the clipboard.
 *
 * The Companion page is often served over plain HTTP on the LAN, where the
 * page is not a secure context and the async Clipboard API is missing. A
 * hidden textarea and the legacy copy command cover that case.
 */
export async function copyText(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    const copied = await navigator.clipboard
      .writeText(text)
      .then(() => true)
      .catch(() => false);
    if (copied) return true;
  }
  return copyWithTextarea(text);
}

/**
 * The legacy copy command, which is the only clipboard writer a page that is
 * not a secure context can use.
 */
function copyWithTextarea(text: string): boolean {
  if (typeof document === "undefined") return false;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.top = "0";
  area.style.left = "-9999px";
  document.body.append(area);
  area.select();
  area.setSelectionRange(0, text.length);
  try {
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    area.remove();
  }
}

/**
 * Offsets of the loaded lines between `from` and `to`, both ends included, in
 * file order.
 *
 * The window holds every loaded line, not only the rendered ones, so a range
 * spans lines the virtualizer has not mounted. An offset outside the window
 * yields no range.
 */
export function offsetRange(
  lines: { offset: number }[],
  from: number,
  to: number,
): number[] {
  const start = lines.findIndex((line) => line.offset === from);
  const end = lines.findIndex((line) => line.offset === to);
  if (start < 0 || end < 0) return [];
  const [first, last] = start <= end ? [start, end] : [end, start];
  return lines.slice(first, last + 1).map((line) => line.offset);
}

export type JsonRow = {
  level: string;
  timestamp: string;
  target: string;
  message: string;
  pretty: string;
};

export type JsonTokenKind =
  "key" | "string" | "number" | "literal" | "punctuation" | "plain";

export type JsonToken = { kind: JsonTokenKind; text: string };

const JSON_WHITESPACE = /\s+/y;
const JSON_NUMBER = /-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/y;
const JSON_LITERAL = /true|false|null/y;

function pushJsonToken(
  tokens: JsonToken[],
  kind: JsonTokenKind,
  text: string,
): void {
  if (!text) return;
  const last = tokens.at(-1);
  if (last && last.kind === kind) {
    last.text += text;
    return;
  }
  tokens.push({ kind, text });
}

/**
 * Splits pretty-printed JSON into tokens for syntax colouring. Every character
 * lands in one token, so joining the tokens gives the input back.
 *
 * A string a colon follows is a key; strings, numbers, `true`, `false` and
 * `null` get their own kinds; brackets, braces, commas and colons are
 * punctuation; whitespace and anything unparseable stay plain.
 */
export function highlightJson(text: string): JsonToken[] {
  const tokens: JsonToken[] = [];
  let index = 0;
  while (index < text.length) {
    const char = text[index] ?? "";
    if (char === '"') {
      let end = index + 1;
      while (end < text.length) {
        const next = text[end];
        if (next === "\\") {
          end += 2;
          continue;
        }
        if (next === '"') {
          end += 1;
          break;
        }
        end += 1;
      }
      end = Math.min(end, text.length);
      let probe = end;
      while (probe < text.length && " \n\r\t".includes(text[probe] ?? "")) {
        probe += 1;
      }
      pushJsonToken(
        tokens,
        text[probe] === ":" ? "key" : "string",
        text.slice(index, end),
      );
      index = end;
      continue;
    }
    JSON_WHITESPACE.lastIndex = index;
    const whitespace = JSON_WHITESPACE.exec(text);
    if (whitespace) {
      pushJsonToken(tokens, "plain", whitespace[0]);
      index += whitespace[0].length;
      continue;
    }
    JSON_NUMBER.lastIndex = index;
    const number = JSON_NUMBER.exec(text);
    if (number) {
      pushJsonToken(tokens, "number", number[0]);
      index += number[0].length;
      continue;
    }
    JSON_LITERAL.lastIndex = index;
    const literal = JSON_LITERAL.exec(text);
    if (literal) {
      pushJsonToken(tokens, "literal", literal[0]);
      index += literal[0].length;
      continue;
    }
    pushJsonToken(
      tokens,
      "{}[],:".includes(char) ? "punctuation" : "plain",
      char,
    );
    index += 1;
  }
  return tokens;
}

function textOf(value: unknown): string {
  return typeof value === "string" ? value : "";
}

export function describeLogLine(line: string): JsonRow | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return null;
  }
  const fields =
    "fields" in parsed &&
    typeof parsed.fields === "object" &&
    parsed.fields !== null
      ? parsed.fields
      : null;
  const message =
    (fields && "message" in fields ? textOf(fields.message) : "") ||
    ("message" in parsed ? textOf(parsed.message) : "");
  return {
    level: "level" in parsed ? textOf(parsed.level) : "",
    timestamp: "timestamp" in parsed ? textOf(parsed.timestamp) : "",
    target: "target" in parsed ? textOf(parsed.target) : "",
    message,
    pretty: JSON.stringify(parsed, null, 2),
  };
}

/**
 * One collapsed log row's geometry in CSS pixels. The row renders
 * `font-mono text-xs` (12 px, 16 px line height) with `px-2 py-1` padding and
 * a 1 px bottom border; the summary button adds `px-1 py-0.5`. A 12 px
 * monospace glyph is about 7.2 px wide. The target chip is capped at 14 rem,
 * so a long target adds at most a line or two.
 */
export const LOG_ROW_CHAR_WIDTH = 7.2;
export const LOG_ROW_LINE_HEIGHT = 16;
export const LOG_ROW_BASE_HEIGHT = 13;
export const LOG_ROW_MIN_CONTENT_HEIGHT = 20;
export const LOG_ROW_HORIZONTAL_PADDING = 24;

/**
 * Estimated height of one collapsed log row at the given viewport width.
 *
 * The row's summary wraps its text at the viewport width, so the estimate
 * counts the line's characters against the characters that fit on one line.
 * The raw JSON text is a little longer than the summary's visible text, which
 * deliberately overestimates a short row by less than a row's height; the
 * virtualizer measures the row when it renders and replaces the estimate.
 */
export function estimateLogRowHeight(line: LogLine, width: number): number {
  const available = Math.max(width - LOG_ROW_HORIZONTAL_PADDING, 40);
  const charsPerLine = Math.max(Math.floor(available / LOG_ROW_CHAR_WIDTH), 1);
  const lines = Math.max(1, Math.ceil(line.text.length / charsPerLine));
  return (
    LOG_ROW_BASE_HEIGHT +
    Math.max(LOG_ROW_MIN_CONTENT_HEIGHT, lines * LOG_ROW_LINE_HEIGHT)
  );
}

/**
 * Scroll offset that keeps an anchored row's top at the same distance from the
 * viewport's top after older lines are prepended.
 *
 * `viewportOffset` is `rowStart - scrollTop` when the anchor was captured, so a
 * row whose top is above the viewport has a negative offset. The new scroll
 * position is the row's new start minus that offset, not plus it: subtracting
 * a negative offset moves the scroll down by the row's overhang.
 */
export function anchorScrollTop(
  anchorStart: number,
  viewportOffset: number,
): number {
  return Math.max(anchorStart - viewportOffset, 0);
}

/**
 * Wall-clock time for the Current run snapshot label, always `HH:MM:SS` in the
 * local time zone so the label reads the same on every device.
 */
export function formatSnapshotTime(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(
    date.getSeconds(),
  )}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}
