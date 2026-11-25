"use client";

import { Button } from "@cloudflare/kumo/components/button";
import { Combobox } from "@cloudflare/kumo/components/combobox";
import { Empty } from "@cloudflare/kumo/components/empty";
import { Input } from "@cloudflare/kumo/components/input";
import { Loader } from "@cloudflare/kumo/components/loader";
import { Text } from "@cloudflare/kumo/components/text";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useTranslations } from "next-intl";
import {
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  LOG_LEVELS,
  LOG_PAGE_LIMIT,
  anchorScrollTop,
  appendPage,
  backwardPage,
  copyText,
  describeLogLine,
  estimateLogRowHeight,
  filterTargetTree,
  formatBytes,
  formatSnapshotTime,
  forwardPage,
  highlightJson,
  newerCursor,
  offsetRange,
  olderCursor,
  prependPage,
  sameFilter,
  startWindow,
  targetTree,
  windowLines,
  type JsonRow,
  type LogWindow,
} from "@/lib/companion-logs";
import {
  classifyCompanionFailure,
  companionErrorCode,
  downloadLogs,
  listLogs,
  readLogPage,
  summarizeLog,
  type LogDirection,
  type LogDownloadEvent,
  type LogFile,
  type LogFilter,
  type LogLevel,
  type LogPage,
  type LogReadRequest,
  type LogSummary,
} from "@/lib/companion";

export type CompanionLogsApi = {
  listLogs: (token: string) => Promise<LogFile[]>;
  readLogPage: (token: string, request: LogReadRequest) => Promise<LogPage>;
  summarizeLog: (
    token: string,
    logId: string,
    onSummary: (summary: LogSummary) => void,
    signal?: AbortSignal,
  ) => Promise<void>;
  downloadLogs: (
    token: string,
    logIds: string[],
    onEvent: (event: LogDownloadEvent) => void,
    signal?: AbortSignal,
  ) => Promise<void>;
};

const liveApi: CompanionLogsApi = {
  listLogs,
  readLogPage,
  summarizeLog,
  downloadLogs,
};

type DownloadState =
  { state: "idle" } | { state: "running"; read: number; total: number };

type LoadState = {
  direction: LogDirection;
  scanned: number;
  size: number;
};

/** One touch gesture that may select the lines it passes. */
type TouchGesture = {
  pointerId: number;
  x: number;
  y: number;
  anchor: number;
  last: number;
  checkbox: boolean;
  select: boolean;
  moved: boolean;
};

/**
 * Distance from the top of the loaded lines, in pixels, that triggers loading
 * the previous page.
 */
const OLDER_LOAD_THRESHOLD = 32;

/**
 * Quiet time after a level toggle or target pick before the filter is applied,
 * so a burst of picks reads the log once.
 */
const FILTER_DEBOUNCE_MS = 500;

/**
 * How long a copy confirmation stays on screen.
 */
const COPIED_RESET_MS = 2000;

/** Movement, in pixels, that turns a touch into a swipe. */
const TOUCH_SLOP = 10;

/** Distance from the list edge, in pixels, that scrolls while swiping. */
const TOUCH_SCROLL_EDGE = 48;

/** Pixels per tick the list scrolls while a finger rests near its edge. */
const TOUCH_SCROLL_STEP = 14;

/** Bundle name when the device does not name one. */
const BUNDLE_NAME = "cadmus-logs.zip";

/**
 * Shortest time a jump shows its locked, scanned state. A fast jump would
 * otherwise replace the window within a frame and read as a flicker.
 */
const MIN_JUMP_MS = 1000;

/**
 * Level colours for rows and pressed filter buttons. The tokens pair a dark
 * shade for the light theme with a light shade for the dark theme.
 */
const LEVEL_TEXT: Record<string, string> = {
  trace: "text-kumo-subtle",
  debug: "text-kumo-info",
  info: "text-kumo-success",
  warn: "text-kumo-warning",
  error: "text-kumo-danger",
};

const LEVEL_PRESSED: Record<LogLevel, string> = {
  trace: "bg-kumo-tint ring ring-kumo-line",
  debug: "bg-kumo-info-tint ring ring-kumo-info/40",
  info: "bg-kumo-success-tint ring ring-kumo-success/40",
  warn: "bg-kumo-warning-tint ring ring-kumo-warning/40",
  error: "bg-kumo-danger-tint ring ring-kumo-danger/40",
};

/** Colours for the highlighted JSON tokens of an expanded entry. */
const JSON_TOKEN_TEXT: Record<string, string> = {
  key: "text-kumo-info",
  string: "text-kumo-success",
  number: "text-kumo-warning",
  literal: "text-kumo-danger",
};

function isAbort(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "name" in error &&
    error.name === "AbortError"
  );
}

function saveBundle(chunks: Uint8Array<ArrayBuffer>[], fileName: string): void {
  const blob = new Blob(chunks, { type: "application/zip" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

export function CompanionLogs({
  token,
  onTokenRejected,
  api = liveApi,
  initialLog = null,
  initialSelected = [],
}: {
  token?: string;
  onTokenRejected?: () => void;
  api?: CompanionLogsApi;
  initialLog?: LogFile | null;
  initialSelected?: string[];
}) {
  const t = useTranslations("companion");
  const [logs, setLogs] = useState<LogFile[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(
    new Set(initialSelected),
  );
  const [opened, setOpened] = useState<LogFile | null>(initialLog);
  const [download, setDownload] = useState<DownloadState>({ state: "idle" });
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const downloadAbort = useRef<AbortController | null>(null);

  const handleFailure = useCallback(
    (error: unknown): string => {
      const failure = classifyCompanionFailure(error);
      if (failure.kind === "unauthorized") {
        onTokenRejected?.();
        return "";
      }
      return failure.message;
    },
    [onTokenRejected],
  );

  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    api
      .listLogs(token)
      .then((next) => {
        if (!cancelled) {
          setLogs(next);
          setLoadError(null);
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setLogs(null);
          setLoadError(handleFailure(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [token, attempt, handleFailure, api]);

  useEffect(
    () => () => {
      downloadAbort.current?.abort();
    },
    [],
  );

  function toggle(id: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function onDownload() {
    if (!token || selected.size === 0 || download.state !== "idle") return;
    setDownloadError(null);
    const abort = new AbortController();
    downloadAbort.current = abort;
    setDownload({ state: "running", read: 0, total: 0 });
    const chunks: Uint8Array<ArrayBuffer>[] = [];
    let fileName = "";
    try {
      await api.downloadLogs(
        token,
        [...selected],
        (event) => {
          if (abort.signal.aborted) return;
          if (event.fileName) fileName = event.fileName;
          if (event.data.byteLength > 0) {
            chunks.push(new Uint8Array(event.data));
          }
          setDownload({
            state: "running",
            read: event.read,
            total: event.total,
          });
        },
        abort.signal,
      );
      if (abort.signal.aborted) return;
      saveBundle(chunks, fileName || BUNDLE_NAME);
    } catch (error) {
      if (!abort.signal.aborted && !isAbort(error)) {
        if (companionErrorCode(error) === "resource_exhausted") {
          setDownloadError(t("logsDownloadBusy"));
        } else {
          setDownloadError(t("logsDownloadError"));
          handleFailure(error);
        }
      }
    } finally {
      if (downloadAbort.current === abort) downloadAbort.current = null;
      if (!abort.signal.aborted) setDownload({ state: "idle" });
    }
  }

  function cancelDownload() {
    downloadAbort.current?.abort();
    downloadAbort.current = null;
    setDownload({ state: "idle" });
  }

  if (opened) {
    return (
      <LogViewer
        token={token}
        log={opened}
        api={api}
        onBack={() => setOpened(null)}
        onTokenRejected={onTokenRejected}
      />
    );
  }

  const busy = download.state === "running";

  return (
    <div className="flex min-h-0 w-full flex-1 flex-col gap-4 p-1">
      <div className="flex flex-wrap items-center gap-3">
        <Text>{t("logsSelected", { count: selected.size })}</Text>
        <Button
          type="button"
          disabled={selected.size === 0 || busy}
          onClick={onDownload}
        >
          {download.state === "running" ? (
            <span className="inline-flex items-center gap-2">
              <Loader size="sm" aria-label={t("logsLoading")} />
              {t("logsDownloading", {
                read: formatBytes(download.read),
                total: formatBytes(download.total),
              })}
            </span>
          ) : (
            t("logsDownload")
          )}
        </Button>
        {busy ? (
          <Button type="button" variant="secondary" onClick={cancelDownload}>
            {t("cancel")}
          </Button>
        ) : null}
        <Button
          type="button"
          variant="secondary"
          disabled={busy}
          onClick={() => {
            setLogs(null);
            setLoadError(null);
            setAttempt((value) => value + 1);
          }}
        >
          {t("logsRefresh")}
        </Button>
      </div>
      {downloadError ? <Text variant="secondary">{downloadError}</Text> : null}
      {loadError ? (
        <div className="flex flex-col gap-1">
          <Text>{t("logsLoadError")}</Text>
          <Text variant="secondary">{loadError}</Text>
        </div>
      ) : null}
      {logs === null && !loadError ? (
        <div className="flex items-center justify-center gap-2 py-8">
          <Loader aria-label={t("logsLoading")} />
          <Text variant="secondary">{t("logsLoading")}</Text>
        </div>
      ) : null}
      {logs && logs.length === 0 ? (
        <Empty size="sm" title={t("logsEmpty")} />
      ) : null}
      {logs && logs.length > 0 ? (
        <ul
          aria-busy={busy}
          className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto"
        >
          {logs.map((log) => (
            <li
              key={log.id}
              className="relative flex flex-wrap items-center gap-3 rounded-xl border border-kumo-line bg-kumo-base p-4 focus-within:ring-2 focus-within:ring-kumo-brand/40 hover:bg-kumo-tint"
            >
              <button
                type="button"
                disabled={busy}
                aria-label={`${t("logsOpen")} ${log.name}`}
                className="absolute inset-0 cursor-pointer rounded-xl focus:outline-hidden focus-visible:ring-2 focus-visible:ring-kumo-brand focus-visible:ring-inset"
                onClick={() => setOpened(log)}
              />
              <input
                type="checkbox"
                className="relative z-10"
                aria-label={log.name}
                disabled={busy}
                checked={selected.has(log.id)}
                onChange={() => toggle(log.id)}
              />
              <div className="flex min-w-0 flex-1 flex-col">
                <Text>
                  {log.kind === "launcher"
                    ? t("logsLauncherKind")
                    : t("logsRunKind")}
                  {log.current ? ` · ${t("logsCurrent")}` : ""}
                </Text>
                <Text variant="secondary">
                  {log.name} · {formatBytes(log.size)}
                  {log.started
                    ? ` · ${new Date(log.started).toLocaleString()}`
                    : ""}
                </Text>
              </div>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function LogViewer({
  token,
  log,
  api,
  onBack,
  onTokenRejected,
}: {
  token?: string;
  log: LogFile;
  api: CompanionLogsApi;
  onBack: () => void;
  onTokenRejected?: () => void;
}) {
  const t = useTranslations("companion");
  const [logWindow, setWindow] = useState<LogWindow | null>(null);
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set());
  const [targets, setTargets] = useState<string[]>([]);
  const [targetQuery, setTargetQuery] = useState("");
  const [text, setText] = useState("");
  const [filter, setFilter] = useState<LogFilter>({
    levels: [],
    targets: [],
    text: "",
  });
  const [busy, setBusy] = useState<LoadState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<LogSummary | null>(null);
  const [snapshot, setSnapshot] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [copiedPicked, setCopiedPicked] = useState<number | null>(null);
  const windowRef = useRef<LogWindow | null>(null);
  const loadGeneration = useRef(0);
  const loadAbort = useRef<AbortController | null>(null);
  const selectionAnchor = useRef<number | null>(null);
  const dragSelect = useRef(true);
  const dragging = useRef(false);
  const scrollIntent = useRef<"end" | "start" | null>("end");
  const settling = useRef(false);
  const pendingAnchor = useRef<{
    offset: number;
    viewportOffset: number;
  } | null>(null);
  const touchGesture = useRef<TouchGesture | null>(null);
  const pointerAt = useRef<{ x: number; y: number } | null>(null);
  const autoScrollDir = useRef(0);
  const autoScrollTimer = useRef<number | null>(null);

  const handleFailure = useCallback(
    (cause: unknown): string => {
      const failure = classifyCompanionFailure(cause);
      if (failure.kind === "unauthorized") {
        onTokenRejected?.();
        return "";
      }
      return failure.message;
    },
    [onTokenRejected],
  );

  const showWindow = useCallback((next: LogWindow) => {
    windowRef.current = next;
    setWindow(next);
  }, []);

  const lines = useMemo(
    () => (logWindow ? windowLines(logWindow) : []),
    [logWindow],
  );
  const pickedLines = useMemo(
    () => lines.filter((line) => picked.has(line.offset)),
    [lines, picked],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const widthRef = useRef(0);
  const getItemKey = useCallback(
    (index: number) => lines[index]?.offset ?? index,
    [lines],
  );
  const estimateSize = useCallback(
    (index: number) => {
      const line = lines[index];
      return line ? estimateLogRowHeight(line, widthRef.current) : 49;
    },
    [lines],
  );
  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => scrollRef.current,
    estimateSize,
    overscan: 8,
    getItemKey,
  });
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const update = () => {
      const width = element.clientWidth;
      if (width === widthRef.current) return;
      widthRef.current = width;
      virtualizer.measure();
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [virtualizer]);

  const abortLoad = useCallback(() => {
    loadGeneration.current += 1;
    loadAbort.current?.abort();
    loadAbort.current = null;
  }, []);

  const runLoad = useCallback(
    async (mode: "jump" | "older" | "newer", direction: LogDirection) => {
      if (!token) return;
      abortLoad();
      const generation = loadGeneration.current;
      const abort = new AbortController();
      loadAbort.current = abort;
      const backward = direction === "backward";
      const existing = mode === "jump" ? null : windowRef.current;
      const startCursor =
        mode === "jump"
          ? null
          : existing
            ? backward
              ? olderCursor(existing)
              : newerCursor(existing)
            : null;
      if (mode !== "jump" && startCursor === null) return;
      setError(null);
      if (mode === "jump") {
        pendingAnchor.current = null;
        scrollIntent.current = backward ? "end" : "start";
      }
      setBusy({
        direction,
        scanned:
          mode === "jump" ? (backward ? log.size : 0) : (startCursor ?? 0),
        size: log.size,
      });
      let working = existing;
      let cursor = startCursor ?? 0;
      let fromEnd = mode === "jump" && backward;
      let gathered = 0;
      const startedAt = Date.now();
      try {
        for (;;) {
          const page = await api.readLogPage(token, {
            logId: log.id,
            fromEnd,
            cursor,
            direction,
            limit: LOG_PAGE_LIMIT,
            filter,
          });
          if (abort.signal.aborted || generation !== loadGeneration.current) {
            return;
          }
          fromEnd = false;
          const entry = backward ? backwardPage(page) : forwardPage(page);
          gathered += entry.page.lines.length;
          working = working
            ? backward
              ? prependPage(working, entry)
              : appendPage(working, entry)
            : startWindow(entry);
          setBusy({
            direction,
            scanned: page.scannedTo,
            size: page.size,
          });
          if (mode === "jump") {
            const wait = MIN_JUMP_MS - (Date.now() - startedAt);
            if (wait > 0) {
              const { promise, resolve } = Promise.withResolvers<void>();
              window.setTimeout(resolve, wait);
              await promise;
              if (
                abort.signal.aborted ||
                generation !== loadGeneration.current
              ) {
                return;
              }
            }
          }
          showWindow(working);
          if (!page.partial || gathered >= LOG_PAGE_LIMIT) break;
          const next = backward ? entry.olderCursor : entry.newerCursor;
          if (next === null) break;
          cursor = next;
        }
        setSnapshot(formatSnapshotTime(new Date()));
      } catch (cause) {
        if (abort.signal.aborted || generation !== loadGeneration.current) {
          return;
        }
        scrollIntent.current = null;
        pendingAnchor.current = null;
        setError(handleFailure(cause));
      } finally {
        if (generation === loadGeneration.current) {
          loadAbort.current = null;
          setBusy(null);
        }
      }
    },
    [
      token,
      log.id,
      log.size,
      filter,
      handleFailure,
      api,
      abortLoad,
      showWindow,
    ],
  );

  const loadEnd = useCallback(() => {
    void runLoad("jump", "backward");
  }, [runLoad]);

  const loadStart = useCallback(() => {
    void runLoad("jump", "forward");
  }, [runLoad]);

  const loadOlder = useCallback(() => {
    const current = windowRef.current;
    if (!current) return;
    const scroller = scrollRef.current;
    const workingLines = windowLines(current);
    const first = scroller
      ? virtualizer.getVirtualItemForOffset(scroller.scrollTop)
      : undefined;
    const line = first ? workingLines[first.index] : undefined;
    if (scroller && first && line) {
      pendingAnchor.current = {
        offset: line.offset,
        viewportOffset: first.start - scroller.scrollTop,
      };
    }
    void runLoad("older", "backward");
  }, [runLoad, virtualizer]);

  const loadNewer = useCallback(() => {
    void runLoad("newer", "forward");
  }, [runLoad]);

  const loadOlderAtTop = useCallback(() => {
    if (
      settling.current ||
      loadAbort.current !== null ||
      touchGesture.current !== null
    ) {
      return;
    }
    const scroller = scrollRef.current;
    const current = windowRef.current;
    if (!scroller || !current || olderCursor(current) === null) return;
    if (scroller.scrollTop > OLDER_LOAD_THRESHOLD) return;
    loadOlder();
  }, [loadOlder]);

  function stopLoad() {
    abortLoad();
    setBusy(null);
  }

  function back() {
    abortLoad();
    setBusy(null);
    onBack();
  }

  useEffect(() => {
    void runLoad("jump", "backward");
  }, [runLoad]);

  useEffect(
    () => () => {
      abortLoad();
    },
    [abortLoad],
  );

  useEffect(() => {
    if (!token) return;
    const controller = new AbortController();
    api
      .summarizeLog(
        token,
        log.id,
        (next) => setSummary(next),
        controller.signal,
      )
      .catch(() => {});
    return () => controller.abort();
  }, [token, log.id, api]);

  useEffect(() => {
    if (busy || lines.length === 0) return;
    const intent = scrollIntent.current;
    if (!intent) return;
    scrollIntent.current = null;
    const scroller = scrollRef.current;
    if (!scroller) return;
    settling.current = true;
    let frame = 0;
    let attempts = 0;
    let previousTarget = -1;
    const step = () => {
      const element = scrollRef.current;
      if (!element) {
        settling.current = false;
        return;
      }
      if (pendingAnchor.current) return;
      const max = element.scrollHeight - element.clientHeight;
      const target = intent === "start" ? 0 : max;
      element.scrollTop = target;
      const settled =
        Math.abs(target - previousTarget) < 1 &&
        Math.abs(element.scrollTop - target) < 1;
      previousTarget = target;
      attempts += 1;
      if (!settled && attempts < 12) {
        frame = requestAnimationFrame(step);
      } else {
        settling.current = false;
      }
    };
    step();
    return () => {
      cancelAnimationFrame(frame);
      settling.current = false;
    };
  }, [lines, busy]);

  useLayoutEffect(() => {
    const anchor = pendingAnchor.current;
    if (!anchor) return;
    const scroller = scrollRef.current;
    if (!scroller) return;
    settling.current = true;
    let frame = 0;
    let attempts = 0;
    let previousTarget = -1;
    let previousTotal = -1;
    let stable = 0;
    const step = () => {
      const element = scrollRef.current;
      if (!element) {
        settling.current = false;
        return;
      }
      const index = lines.findIndex((line) => line.offset === anchor.offset);
      if (index < 0) {
        settling.current = false;
        return;
      }
      const placement = virtualizer.getOffsetForIndex(index, "start");
      if (!placement) {
        settling.current = false;
        return;
      }
      const total = virtualizer.getTotalSize();
      const max = Math.max(total - element.clientHeight, 0);
      const target = Math.min(
        Math.max(anchorScrollTop(placement[0], anchor.viewportOffset), 0),
        max,
      );
      const targetChanged = Math.abs(target - previousTarget) >= 1;
      const scrollMismatch =
        touchGesture.current === null &&
        Math.abs(element.scrollTop - target) >= 1;
      if (targetChanged || scrollMismatch) {
        element.scrollTop = target;
      }
      const settled =
        !targetChanged &&
        Math.abs(total - previousTotal) < 1 &&
        (touchGesture.current !== null ||
          Math.abs(element.scrollTop - target) < 1);
      previousTarget = target;
      previousTotal = total;
      stable = settled ? stable + 1 : 0;
      attempts += 1;
      if (stable < 4 && attempts < 60) {
        frame = requestAnimationFrame(step);
      } else {
        settling.current = false;
        if (busy === null) pendingAnchor.current = null;
      }
    };
    step();
    return () => {
      cancelAnimationFrame(frame);
      settling.current = false;
    };
  }, [lines, virtualizer, busy]);

  const older = logWindow ? olderCursor(logWindow) : null;
  const newer = logWindow ? newerCursor(logWindow) : null;
  const tree = useMemo(() => targetTree(summary?.targets ?? []), [summary]);
  const selectedTargets = useMemo(
    () =>
      targets.map(
        (value) =>
          tree.find((node) => node.value === value) ?? { value, lines: 0 },
      ),
    [tree, targets],
  );
  const visibleTargets = useMemo(
    () => filterTargetTree(tree, targetQuery),
    [tree, targetQuery],
  );

  const locked = busy !== null;

  function sameSelection(
    applied: LogFilter,
    nextLevels: Set<LogLevel>,
    nextTargets: string[],
  ): boolean {
    return (
      applied.levels.length === nextLevels.size &&
      applied.levels.every((level) => nextLevels.has(level)) &&
      applied.targets.length === nextTargets.length &&
      applied.targets.every((target) => nextTargets.includes(target))
    );
  }

  useEffect(() => {
    if (sameSelection(filter, levels, targets)) return;
    const nextLevels = [...levels].sort();
    const nextTargets = [...targets];
    const timer = window.setTimeout(() => {
      setFilter((current) => ({
        levels: nextLevels,
        targets: nextTargets,
        text: current.text,
      }));
    }, FILTER_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [filter, levels, targets]);

  useEffect(() => {
    if (copiedPicked === null) return;
    const timer = window.setTimeout(
      () => setCopiedPicked(null),
      COPIED_RESET_MS,
    );
    return () => window.clearTimeout(timer);
  }, [copiedPicked]);

  useEffect(() => {
    function endDrag() {
      dragging.current = false;
    }
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    return () => {
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
    };
  }, []);

  function applyText() {
    setFilter((current) => {
      const next = { ...current, text };
      return sameFilter(current, next) ? current : next;
    });
  }

  function applyOffsets(
    current: Set<number>,
    offsets: number[],
    select: boolean,
  ): Set<number> {
    const next = new Set(current);
    for (const offset of offsets) {
      if (select) next.add(offset);
      else next.delete(offset);
    }
    return next;
  }

  function togglePicked(offset: number) {
    selectionAnchor.current = offset;
    setPicked((current) =>
      applyOffsets(current, [offset], !current.has(offset)),
    );
  }

  function extendSelection(offset: number) {
    const anchor = selectionAnchor.current;
    if (!dragging.current || anchor === null) return;
    const offsets = offsetRange(lines, anchor, offset);
    setPicked((current) => applyOffsets(current, offsets, dragSelect.current));
  }

  function toggleSelecting() {
    setSelecting((current) => !current);
    setPicked(new Set());
    setCopiedPicked(null);
    selectionAnchor.current = null;
    dragging.current = false;
    touchGesture.current = null;
    setAutoScroll(0);
  }

  function offsetAtPoint(x: number, y: number): number | null {
    const row = document
      .elementFromPoint(x, y)
      ?.closest<HTMLElement>("[data-log-offset]");
    const value = Number(row?.dataset.logOffset);
    return Number.isFinite(value) ? value : null;
  }

  function selectUpTo(gesture: TouchGesture, offset: number) {
    const offsets = offsetRange(lines, gesture.last, offset);
    gesture.last = offset;
    if (offsets.length === 0) return;
    setPicked((current) => applyOffsets(current, offsets, gesture.select));
  }

  function setAutoScroll(dir: number) {
    autoScrollDir.current = dir;
    if (dir === 0) {
      if (autoScrollTimer.current !== null) {
        window.clearInterval(autoScrollTimer.current);
        autoScrollTimer.current = null;
      }
      return;
    }
    if (autoScrollTimer.current !== null) return;
    autoScrollTimer.current = window.setInterval(() => {
      const gesture = touchGesture.current;
      const point = pointerAt.current;
      const scroller = scrollRef.current;
      if (!gesture || !point || !scroller || autoScrollDir.current === 0) {
        setAutoScroll(0);
        return;
      }
      const before = scroller.scrollTop;
      scroller.scrollTop = before + autoScrollDir.current * TOUCH_SCROLL_STEP;
      if (scroller.scrollTop === before) {
        setAutoScroll(0);
        return;
      }
      if (!gesture.checkbox) return;
      const offset = offsetAtPoint(point.x, point.y);
      if (offset !== null) selectUpTo(gesture, offset);
    }, 16);
  }

  useEffect(
    () => () => {
      if (autoScrollTimer.current !== null) {
        window.clearInterval(autoScrollTimer.current);
        autoScrollTimer.current = null;
      }
    },
    [],
  );

  function updateAutoScroll(y: number) {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const rect = scroller.getBoundingClientRect();
    if (y < rect.top + TOUCH_SCROLL_EDGE) setAutoScroll(-1);
    else if (y > rect.bottom - TOUCH_SCROLL_EDGE) setAutoScroll(1);
    else setAutoScroll(0);
  }

  function startTouch(
    event: ReactPointerEvent<HTMLDivElement>,
    offset: number,
    target: HTMLElement,
  ) {
    const checkbox = target.closest("[data-log-checkbox]") !== null;
    touchGesture.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      anchor: offset,
      last: offset,
      checkbox,
      select: !picked.has(offset),
      moved: false,
    };
    if (checkbox) event.currentTarget.setPointerCapture(event.pointerId);
  }

  function onListPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (!selecting || locked) return;
    const target = event.target as HTMLElement;
    const row = target.closest<HTMLElement>("[data-log-offset]");
    const offset = Number(row?.dataset.logOffset);
    if (!row || !Number.isFinite(offset)) return;
    if (event.pointerType === "touch") {
      startTouch(event, offset, target);
      return;
    }
    event.preventDefault();
    if (event.shiftKey && selectionAnchor.current !== null) {
      const offsets = offsetRange(lines, selectionAnchor.current, offset);
      const select = !picked.has(offset);
      setPicked((current) => applyOffsets(current, offsets, select));
      return;
    }
    const select = !picked.has(offset);
    selectionAnchor.current = offset;
    dragSelect.current = select;
    dragging.current = true;
    setPicked((current) => applyOffsets(current, [offset], select));
  }

  function onListPointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    const gesture = touchGesture.current;
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    if (!gesture.moved) {
      if (
        Math.abs(event.clientX - gesture.x) < TOUCH_SLOP &&
        Math.abs(event.clientY - gesture.y) < TOUCH_SLOP
      ) {
        return;
      }
      gesture.moved = true;
    }
    pointerAt.current = { x: event.clientX, y: event.clientY };
    if (!gesture.checkbox) return;
    updateAutoScroll(event.clientY);
    const offset = offsetAtPoint(event.clientX, event.clientY);
    if (offset !== null) selectUpTo(gesture, offset);
  }

  function endTouch(event: ReactPointerEvent<HTMLDivElement>, toggle: boolean) {
    const gesture = touchGesture.current;
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    touchGesture.current = null;
    pointerAt.current = null;
    setAutoScroll(0);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (toggle && !gesture.moved) {
      setPicked((current) =>
        applyOffsets(current, [gesture.anchor], !current.has(gesture.anchor)),
      );
    }
  }

  function onListPointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    endTouch(event, true);
  }

  function onListPointerCancel(event: ReactPointerEvent<HTMLDivElement>) {
    endTouch(event, false);
  }

  async function copyPicked() {
    if (pickedLines.length === 0) return;
    const copied = await copyText(
      pickedLines.map((line) => line.text).join("\n"),
    );
    if (copied) setCopiedPicked(pickedLines.length);
  }

  const scannedBytes = busy
    ? Math.max(
        busy.direction === "backward" ? busy.size - busy.scanned : busy.scanned,
        0,
      )
    : 0;

  return (
    <div className="flex min-h-0 w-full flex-1 flex-col gap-3 p-1">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" onClick={back}>
          {t("logsBack")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          disabled={locked}
          onClick={loadEnd}
        >
          {t("logsJumpEnd")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          disabled={locked}
          onClick={loadStart}
        >
          {t("logsJumpStart")}
        </Button>
        {log.current ? (
          <Button
            type="button"
            variant="secondary"
            disabled={locked}
            onClick={loadEnd}
          >
            {t("logsRefresh")}
          </Button>
        ) : null}
        {busy ? (
          <Button type="button" variant="secondary" onClick={stopLoad}>
            {t("logsStop")}
          </Button>
        ) : null}
      </div>
      {busy ? (
        <div role="status" className="flex items-center gap-2">
          <Loader size="sm" aria-label={t("logsLoading")} />
          <Text variant="secondary">
            {t("logsScanned", {
              scanned: formatBytes(scannedBytes),
              size: formatBytes(busy.size),
            })}
          </Text>
        </div>
      ) : null}
      {log.current && snapshot ? (
        <Text variant="secondary">{t("logsSnapshot", { time: snapshot })}</Text>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        {LOG_LEVELS.map((level) => {
          const pressed = levels.has(level);
          return (
            <Button
              key={level}
              type="button"
              variant={pressed ? "ghost" : "secondary"}
              aria-pressed={pressed}
              disabled={locked}
              className={pressed ? LEVEL_PRESSED[level] : undefined}
              onClick={() =>
                setLevels((current) => {
                  const next = new Set(current);
                  if (next.has(level)) next.delete(level);
                  else next.add(level);
                  return next;
                })
              }
            >
              <span className={pressed ? LEVEL_TEXT[level] : undefined}>
                {level.toUpperCase()}
              </span>
            </Button>
          );
        })}
        <div inert={locked} aria-disabled={locked}>
          <Combobox
            multiple
            items={visibleTargets}
            value={selectedTargets}
            onValueChange={(value) =>
              setTargets(value.map((node) => node.value))
            }
            onInputValueChange={setTargetQuery}
            itemToStringLabel={(item) => item.value}
            isItemEqualToValue={(a, b) => a.value === b.value}
            filter={() => true}
          >
            <Combobox.TriggerMultipleWithInput
              aria-label={t("logsTarget")}
              placeholder={t("logsTargetSearch")}
              value={selectedTargets}
              className="w-80"
              renderItem={(node) => (
                <Combobox.Chip
                  key={node.value}
                  removeLabel={t("logsRemoveTarget")}
                  title={node.value}
                >
                  {node.value}
                </Combobox.Chip>
              )}
            />
            <Combobox.Content>
              <Combobox.List>
                {(item) => (
                  <Combobox.Item
                    key={item.value}
                    value={item}
                    disabled={locked}
                    aria-label={`${item.value} (${item.lines})`}
                  >
                    <span className="flex items-baseline justify-between gap-2">
                      <span className="break-all">{item.value}</span>
                      <span className="text-kumo-muted text-xs">
                        {item.lines}
                      </span>
                    </span>
                  </Combobox.Item>
                )}
              </Combobox.List>
              <Combobox.Empty>{t("logsTargetsEmpty")}</Combobox.Empty>
            </Combobox.Content>
          </Combobox>
        </div>
        <Input
          aria-label={t("logsText")}
          placeholder={t("logsText")}
          value={text}
          disabled={locked}
          onChange={(event) => setText(event.target.value)}
          onBlur={applyText}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            applyText();
          }}
        />
      </div>
      {summary ? (
        <Text variant="secondary">
          {summary.done
            ? t("logsSummary", {
                lines: summary.lines,
                size: formatBytes(summary.size),
                first: summary.firstTimestamp || "—",
                last: summary.lastTimestamp || "—",
              })
            : t("logsScanning", {
                scanned: formatBytes(summary.scanned),
                size: formatBytes(summary.size),
              })}
        </Text>
      ) : null}
      {error ? <Text variant="secondary">{error}</Text> : null}
      <div
        ref={scrollRef}
        onScroll={loadOlderAtTop}
        onPointerDown={onListPointerDown}
        onPointerMove={onListPointerMove}
        onPointerUp={onListPointerUp}
        onPointerCancel={onListPointerCancel}
        aria-busy={busy !== null}
        style={{ overflowAnchor: "none" }}
        className="min-h-0 flex-1 overflow-y-auto rounded-xl border border-kumo-line bg-kumo-base font-mono text-xs"
      >
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            position: "relative",
          }}
        >
          {virtualizer.getVirtualItems().map((item) => {
            const line = lines[item.index];
            if (!line) return null;
            const row = describeLogLine(line.text);
            const open = expanded.has(line.offset);
            const cut =
              line.cutFrom > 0
                ? t("logsLineCut", { size: formatBytes(line.cutFrom) })
                : "";
            return (
              <div
                key={item.key}
                data-index={item.index}
                data-log-offset={line.offset}
                ref={(node) => {
                  if (node) virtualizer.measureElement(node);
                }}
                onPointerEnter={
                  selecting && !locked
                    ? () => extendSelection(line.offset)
                    : undefined
                }
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${item.start}px)`,
                }}
                className={`border-b border-kumo-line px-2 py-1 ${
                  selecting ? "cursor-pointer select-none" : ""
                }`}
              >
                <div className="flex items-start gap-1">
                  {selecting ? (
                    <div
                      data-log-checkbox
                      style={{ touchAction: "none" }}
                      className="flex w-11 shrink-0 self-stretch justify-center"
                    >
                      <input
                        type="checkbox"
                        className="pointer-events-none mt-1"
                        aria-label={line.text.slice(0, 120)}
                        disabled={locked}
                        checked={picked.has(line.offset)}
                        onChange={() => togglePicked(line.offset)}
                      />
                    </div>
                  ) : null}
                  <div className="min-w-0 flex-1">
                    {row ? (
                      selecting ? (
                        <div className="flex w-full flex-wrap items-baseline gap-2 px-1 py-0.5">
                          <LogLineSummary row={row} />
                        </div>
                      ) : (
                        <JsonLine
                          row={row}
                          open={open}
                          disabled={locked}
                          cut={cut}
                          onToggle={() =>
                            setExpanded((current) => {
                              const next = new Set(current);
                              if (next.has(line.offset))
                                next.delete(line.offset);
                              else next.add(line.offset);
                              return next;
                            })
                          }
                          showEntry={t("logsShowEntry")}
                          hideEntry={t("logsHideEntry")}
                          copy={t("logsCopy")}
                          copied={t("logsCopied")}
                        />
                      )
                    ) : (
                      <span className="whitespace-pre-wrap break-all">
                        {line.text}
                      </span>
                    )}
                  </div>
                  {cut ? (
                    <span className="shrink-0 rounded bg-kumo-tint px-1 py-0.5 text-kumo-muted">
                      {cut}
                    </span>
                  ) : null}
                </div>
              </div>
            );
          })}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant={selecting ? "primary" : "secondary"}
          aria-pressed={selecting}
          disabled={locked}
          onClick={toggleSelecting}
        >
          {selecting ? t("logsDone") : t("logsSelectLines")}
        </Button>
        {selecting ? (
          <>
            <Button
              type="button"
              disabled={locked || pickedLines.length === 0}
              onClick={() => void copyPicked()}
            >
              {t("logsCopyLines", { count: pickedLines.length })}
            </Button>
            {copiedPicked === null ? null : (
              <Text variant="secondary" role="status">
                {t("logsCopiedLines", { count: copiedPicked })}
              </Text>
            )}
          </>
        ) : null}
        <Button
          type="button"
          variant="secondary"
          disabled={locked || older === null}
          onClick={loadOlder}
        >
          {t("logsLoadOlder")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          disabled={locked || newer === null}
          onClick={loadNewer}
        >
          {t("logsLoadNewer")}
        </Button>
        {older === null && lines.length === 0 && !busy ? (
          <Text variant="secondary">{t("logsNoLines")}</Text>
        ) : null}
      </div>
    </div>
  );
}

function LogLineSummary({ row }: { row: JsonRow }) {
  return (
    <>
      <span
        className={`font-semibold ${LEVEL_TEXT[row.level.toLowerCase()] ?? ""}`}
      >
        {row.level}
      </span>
      <span className="text-kumo-muted">{row.timestamp}</span>
      {row.target ? (
        <span
          title={row.target}
          className="inline-block max-w-56 truncate rounded-full bg-kumo-fill px-2 py-0.5 align-baseline font-mono text-kumo-default"
        >
          {row.target}
        </span>
      ) : null}
      <span className="break-all">{row.message}</span>
    </>
  );
}

function JsonLine({
  row,
  open,
  disabled,
  cut,
  onToggle,
  showEntry,
  hideEntry,
  copy,
  copied,
}: {
  row: JsonRow;
  open: boolean;
  disabled: boolean;
  cut: string;
  onToggle: () => void;
  showEntry: string;
  hideEntry: string;
  copy: string;
  copied: string;
}) {
  const [didCopy, setDidCopy] = useState(false);
  const tokens = useMemo(() => highlightJson(row.pretty), [row.pretty]);

  useEffect(() => {
    if (!didCopy) return;
    const timer = window.setTimeout(() => setDidCopy(false), COPIED_RESET_MS);
    return () => window.clearTimeout(timer);
  }, [didCopy]);

  async function onCopy() {
    if (await copyText(row.pretty)) setDidCopy(true);
  }

  return (
    <div className="flex flex-col gap-1">
      <button
        type="button"
        disabled={disabled}
        className="flex w-full cursor-pointer flex-wrap items-baseline gap-2 rounded-md px-1 py-0.5 text-left hover:bg-kumo-tint"
        onClick={onToggle}
        aria-expanded={open}
      >
        <span aria-hidden className="text-kumo-muted">
          {open ? "▾" : "▸"}
        </span>
        <LogLineSummary row={row} />
        <span className="ml-auto text-kumo-muted underline decoration-dotted">
          {open ? hideEntry : showEntry}
        </span>
      </button>
      {open ? (
        <div className="flex flex-col gap-1 rounded-lg border border-kumo-line bg-kumo-tint/50 p-2">
          <div className="flex justify-end">
            <Button
              type="button"
              size="xs"
              variant="secondary"
              onClick={() => void onCopy()}
            >
              {didCopy ? copied : copy}
            </Button>
          </div>
          {cut ? <Text variant="secondary">{cut}</Text> : null}
          <pre className="break-all whitespace-pre-wrap">
            {tokens.map((token, index) =>
              token.kind === "plain" ? (
                token.text
              ) : (
                <span
                  key={`${index}:${token.kind}`}
                  className={JSON_TOKEN_TEXT[token.kind] ?? ""}
                >
                  {token.text}
                </span>
              ),
            )}
          </pre>
        </div>
      ) : null}
    </div>
  );
}
