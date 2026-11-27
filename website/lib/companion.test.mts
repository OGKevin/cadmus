import assert from "node:assert/strict";
import test from "node:test";
import {
  classifyCompanionFailure,
  companionErrorCode,
  downloadLogs,
  fillColorChoice,
  getFile,
  isUnauthorized,
  listLogs,
  listSettings,
  readLogPage,
  shouldReloadAfterLibraryChange,
  summarizeLog,
  takeQueuedUpdate,
  updateSetting,
  withSettingValue,
  type Category,
  type LogDownloadEvent,
  type LogSummary,
  type Setting,
  type SettingChoice,
  type SettingInput,
} from "./companion.ts";
import { shouldReplaceSessionToken } from "./companion-token.ts";

const whites = { id: "#ffffff", label: "White" };
const blacks = { id: "#000000", label: "Black" };

function fillSetting(choice: SettingChoice): Setting {
  return {
    id: "IntermissionFillColor",
    protoId: { kind: "SETTING_IDENTITY_KIND_INTERMISSION_FILL_COLOR" },
    label: "Fill Color",
    kind: "choice",
    choice,
  };
}

test("fill color keeps White, Black, and the current hex", () => {
  const dirty: SettingChoice = {
    selected: "#804020",
    options: [
      whites,
      blacks,
      { id: "#aa0000", label: "#aa0000" },
      { id: "#00aa00", label: "#00aa00" },
      { id: "#804020", label: "#804020" },
    ],
  };

  assert.deepEqual(fillColorChoice(dirty).options, [
    whites,
    blacks,
    { id: "#804020", label: "#804020" },
  ]);

  const setting = withSettingValue(fillSetting(dirty), "#112233");
  assert.equal(setting.kind, "choice");
  if (setting.kind !== "choice") return;
  assert.deepEqual(setting.choice.options, [
    whites,
    blacks,
    { id: "#112233", label: "#112233" },
  ]);

  const white = withSettingValue(setting, "#ffffff");
  assert.equal(white.kind, "choice");
  if (white.kind !== "choice") return;
  assert.deepEqual(white.choice.options, [whites, blacks]);
});

test("withSettingValue moves the library protoId key with its path", () => {
  const setting: Setting = {
    id: "LibraryInfo:0:/mnt/onboard/books",
    protoId: {
      kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
      index: 0,
      key: "/mnt/onboard/books",
    },
    label: "Books",
    kind: "library",
    library: {
      name: "Books",
      path: { path: "/mnt/onboard/books" },
      finished: { selected: "Close", options: [] },
    },
  };
  const moved = withSettingValue(setting, {
    name: "Moved",
    path: { path: "/mnt/onboard/moved" },
    finished: { selected: "Close", options: [] },
  });
  assert.equal(moved.kind, "library");
  if (moved.kind !== "library") return;
  assert.deepEqual(moved.protoId, {
    kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
    index: 0,
    key: "/mnt/onboard/moved",
  });
});

type FetchCall = { url: unknown; init: RequestInit };

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

async function withStubbedFetch(
  respond: () => Response,
  run: (calls: FetchCall[]) => Promise<void>,
): Promise<void> {
  const calls: FetchCall[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (input: unknown, init?: RequestInit) => {
    calls.push({ url: input, init: init ?? {} });
    return respond();
  }) as typeof fetch;
  try {
    await run(calls);
  } finally {
    globalThis.fetch = original;
  }
}

const listSample = {
  categories: [
    {
      id: "CATEGORY_ID_GENERAL",
      label: "General",
      settings: [
        {
          id: { kind: "SETTING_IDENTITY_KIND_LOCALE" },
          label: "Language",
          value: {
            choice: {
              selected: "en-GB",
              options: [
                { id: "en-GB", label: "en-GB" },
                { id: "fr", label: "fr" },
              ],
            },
          },
        },
        {
          id: { kind: "SETTING_IDENTITY_KIND_NTP_SERVER" },
          label: "NTP Server",
          value: {
            text: {
              text: "time.cloudflare.com",
              mode: "TEXT_MODE_HOSTNAME",
            },
          },
        },
      ],
    },
    {
      id: "CATEGORY_ID_LIBRARIES",
      label: "Libraries",
      settings: [
        {
          id: {
            kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
            index: 0,
            key: "/mnt/onboard/books",
          },
          label: "Books",
          value: {
            library: {
              name: "Books",
              path: { path: "/mnt/onboard/books" },
              finished: { selected: "Close" },
            },
          },
        },
      ],
    },
    {
      id: "CATEGORY_ID_READER",
      label: "Reader",
      settings: [
        {
          id: {
            kind: "SETTING_IDENTITY_KIND_REFRESH_RATE_BY_KIND",
            key: "EXTENSION_PDF",
          },
          label: "PDF",
          value: {
            refreshRate: {
              regular: 8,
              inverted: 2,
              byKind: [],
              available: ["EXTENSION_PDF"],
            },
          },
        },
      ],
    },
  ],
};

test("settings rows parse into friendly ids and category ids", async () => {
  await withStubbedFetch(
    () => jsonResponse(listSample),
    async (calls) => {
      assert.equal(calls.length, 0);
      const categories = await listSettings("1234");
      assert.equal(calls.length, 1);
      const [general, libraries, reader] = categories;
      assert.ok(general && libraries && reader);
      assert.equal(general.id, "general");
      const [locale, ntp] = general.settings;
      assert.ok(locale && ntp);
      assert.equal(locale.id, "Locale");
      assert.equal(locale.kind, "choice");
      assert.equal(ntp.id, "NtpServer");
      assert.equal(ntp.kind, "text");
      assert.equal(ntp.mode, "hostname");
      assert.equal(ntp.protoId.kind, "SETTING_IDENTITY_KIND_NTP_SERVER");
      assert.equal(libraries.id, "libraries");
      const [library] = libraries.settings;
      assert.ok(library);
      assert.equal(library.id, "LibraryInfo:0:/mnt/onboard/books");
      assert.equal(library.kind, "library");
      assert.deepEqual(library.protoId, {
        kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
        index: 0,
        key: "/mnt/onboard/books",
      });
      const [byKind] = reader.settings;
      assert.ok(byKind);
      assert.equal(byKind.id, "RefreshRateByKind:EXTENSION_PDF");
      assert.equal(byKind.kind, "refreshRate");
    },
  );
});

test("updateSetting sends the id message with the text mode", async () => {
  const setting: Setting = {
    id: "NtpServer",
    protoId: { kind: "SETTING_IDENTITY_KIND_NTP_SERVER" },
    label: "NTP Server",
    kind: "text",
    mode: "hostname",
    text: "time.cloudflare.com",
  };
  await withStubbedFetch(
    () => jsonResponse({}),
    async (calls) => {
      await updateSetting("1234", setting, "pool.ntp.test");
      const [call] = calls;
      assert.ok(call);
      assert.equal(
        new Headers(call.init.headers).get("content-type"),
        "application/json",
      );
      assert.deepEqual(JSON.parse(String(call.init.body)), {
        id: { kind: "SETTING_IDENTITY_KIND_NTP_SERVER" },
        value: { text: { text: "pool.ntp.test", mode: "TEXT_MODE_HOSTNAME" } },
      });
    },
  );
});

function connectFrame(flags: number, payload: string): Uint8Array {
  const bytes = new TextEncoder().encode(payload);
  const frame = new Uint8Array(5 + bytes.length);
  frame[0] = flags;
  new DataView(frame.buffer).setUint32(1, bytes.length);
  frame.set(bytes, 5);
  return frame;
}

function streamResponse(frames: Uint8Array[]): Response {
  return new Response(new Blob(frames), {
    status: 200,
    headers: { "content-type": "application/connect+json" },
  });
}

test("getFile decodes framed connect streaming chunks", async () => {
  const response = streamResponse([
    connectFrame(
      0,
      JSON.stringify({ data: btoa("Hel"), contentType: "text/plain" }),
    ),
    connectFrame(0, JSON.stringify({ data: btoa("lo!") })),
    connectFrame(2, "{}"),
  ]);
  await withStubbedFetch(
    () => response,
    async (calls) => {
      const blob = await getFile("1234", "notes.txt");
      assert.equal(await blob.text(), "Hello!");
      assert.equal(blob.type, "text/plain");
      const [call] = calls;
      assert.ok(call);
      assert.equal(
        new Headers(call.init.headers).get("content-type"),
        "application/connect+json",
      );
      const request = call.init.body as Uint8Array;
      const length = new DataView(request.buffer).getUint32(1);
      assert.deepEqual(
        JSON.parse(new TextDecoder().decode(request.subarray(5, 5 + length))),
        {
          path: "notes.txt",
        },
      );
    },
  );
});

test("getFile surfaces in-stream connect errors", async () => {
  const response = streamResponse([
    connectFrame(
      2,
      JSON.stringify({ error: { code: "not_found", message: "no such file" } }),
    ),
  ]);
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(getFile("1234", "missing.txt"), /not_found/);
    },
  );
});

test("settings rows with unparseable ids fail loudly", async () => {
  await withStubbedFetch(
    () =>
      jsonResponse({
        categories: [
          {
            id: "CATEGORY_ID_GENERAL",
            label: "General",
            settings: [
              {
                id: "Locale",
                label: "Language",
                value: { choice: { selected: "en-GB", options: [] } },
              },
            ],
          },
        ],
      }),
    async () => {
      await assert.rejects(listSettings("1234"), /SettingId/);
    },
  );
});

test("settings categories with unparseable ids fail loudly", async () => {
  await withStubbedFetch(
    () =>
      jsonResponse({
        categories: [{ id: "libraries", label: "Libraries", settings: [] }],
      }),
    async () => {
      await assert.rejects(listSettings("1234"), /CategoryId/);
    },
  );
});

test("settings rows with unknown kinds are skipped with a warning", async () => {
  await withStubbedFetch(
    () =>
      jsonResponse({
        categories: [
          {
            id: "CATEGORY_ID_GENERAL",
            label: "General",
            settings: [
              {
                id: { kind: "SETTING_IDENTITY_KIND_TIME_MACHINE" },
                label: "Time Machine",
                value: { toggle: true },
              },
            ],
          },
        ],
      }),
    async () => {
      const categories = await listSettings("1234");
      assert.deepEqual(categories[0]?.settings, []);
    },
  );
});

test("requests keep the Connect error message and status", async () => {
  await withStubbedFetch(
    () =>
      new Response(
        JSON.stringify({
          code: "internal",
          message: "library path is missing",
        }),
        { status: 500, headers: { "content-type": "application/json" } },
      ),
    async () => {
      await assert.rejects(listSettings("1234"), {
        message: "library path is missing",
        status: 500,
      });
    },
  );
});

test("401 responses keep the message and are reported as unauthorized", async () => {
  await withStubbedFetch(
    () =>
      new Response(
        JSON.stringify({ code: "unauthenticated", message: "bad token" }),
        { status: 401, headers: { "content-type": "application/json" } },
      ),
    async () => {
      await assert.rejects(
        listSettings("1234"),
        (error: unknown) =>
          isUnauthorized(error) &&
          error instanceof Error &&
          error.message === "bad token",
      );
    },
  );
});

test("getFile keeps the content type of a zero-byte chunk", async () => {
  const response = streamResponse([
    connectFrame(0, JSON.stringify({ contentType: "image/png" })),
    connectFrame(2, "{}"),
  ]);
  await withStubbedFetch(
    () => response,
    async () => {
      const blob = await getFile("1234", "empty.png");
      assert.equal(blob.size, 0);
      assert.equal(blob.type, "image/png");
    },
  );
});

test("queued updates skip settings that no longer exist", () => {
  const choice: SettingChoice = {
    selected: "#ffffff",
    options: [whites, blacks],
  };
  const categories: Category[] = [
    {
      id: "CATEGORY_ID_GENERAL",
      label: "General",
      settings: [{ ...fillSetting(choice), id: "kept" }],
    },
  ];
  const queue = new Map<string, SettingInput>([
    ["removed", true],
    ["kept", false],
    ["later", true],
  ]);

  assert.deepEqual(takeQueuedUpdate(queue, categories), ["kept", false]);
  assert.deepEqual([...queue.keys()], ["later"]);
  assert.equal(takeQueuedUpdate(queue, null), undefined);
  assert.equal(queue.size, 0);
});

test("playground token replaces blank and previously prefilled values only", () => {
  assert.equal(shouldReplaceSessionToken("", null), true);
  assert.equal(shouldReplaceSessionToken("  ", "old"), true);
  assert.equal(shouldReplaceSessionToken(undefined, null), true);
  assert.equal(shouldReplaceSessionToken("old", "old"), true);
  assert.equal(shouldReplaceSessionToken("typed", "old"), false);
  assert.equal(shouldReplaceSessionToken("typed", null), false);
});

test("401 failures are unauthorized and other failures keep their message", () => {
  assert.deepEqual(
    classifyCompanionFailure(
      Object.assign(new Error("Token rejected"), { status: 401 }),
    ),
    { kind: "unauthorized" },
  );
  assert.deepEqual(
    classifyCompanionFailure(new Error("Library path is missing")),
    {
      kind: "visible",
      message: "Library path is missing",
    },
  );
  assert.deepEqual(classifyCompanionFailure("Companion is offline"), {
    kind: "visible",
    message: "Companion is offline",
  });
});

test("only removing a library reloads the settings view", () => {
  assert.equal(shouldReloadAfterLibraryChange("remove"), true);
  assert.equal(shouldReloadAfterLibraryChange("update"), false);
});

test("log list parses run and launcher logs with proto3 defaults", async () => {
  await withStubbedFetch(
    () =>
      jsonResponse({
        logs: [
          {
            id: "run-1",
            kind: "LOG_KIND_RUN",
            name: "cadmus-r1.json",
            size: "2048",
            modified: "2025-11-14T22:13:20Z",
            started: "2025-11-14T22:13:19Z",
            current: true,
            runId: "r1",
          },
          {
            id: "launcher",
            kind: "LOG_KIND_LAUNCHER",
            name: "info.log",
            size: 10,
            modified: "2025-11-14T21:13:20.000000Z",
          },
        ],
      }),
    async (calls) => {
      assert.deepEqual(await listLogs("1234"), [
        {
          id: "run-1",
          kind: "run",
          name: "cadmus-r1.json",
          size: 2048,
          modified: "2025-11-14T22:13:20Z",
          started: "2025-11-14T22:13:19Z",
          current: true,
          runId: "r1",
        },
        {
          id: "launcher",
          kind: "launcher",
          name: "info.log",
          size: 10,
          modified: "2025-11-14T21:13:20.000000Z",
          started: "",
          current: false,
          runId: "",
        },
      ]);
      const [call] = calls;
      assert.ok(call);
      assert.equal(call.url, "/cadmus.api.public.v1.Logs/List");
    },
  );
});

test("log reads send level names, targets, direction and cursor", async () => {
  await withStubbedFetch(
    () =>
      jsonResponse({
        lines: [{ offset: "5", text: "hi", cutFrom: "9000" }],
        start: "5",
        end: "7",
        scannedTo: "7",
        size: "100",
        partial: true,
      }),
    async (calls) => {
      const page = await readLogPage("1234", {
        logId: "run-1",
        fromEnd: false,
        cursor: 5,
        direction: "backward",
        limit: 200,
        filter: {
          levels: ["warn", "error"],
          targets: ["cadmus", "cadmus_core::device"],
          text: "",
        },
      });
      assert.deepEqual(page, {
        lines: [{ offset: 5, text: "hi", cutFrom: 9000 }],
        start: 5,
        end: 7,
        scannedTo: 7,
        size: 100,
        partial: true,
      });
      const [call] = calls;
      assert.ok(call);
      assert.deepEqual(JSON.parse(String(call.init.body)), {
        logId: "run-1",
        fromEnd: false,
        cursor: 5,
        direction: "READ_DIRECTION_BACKWARD",
        limit: 200,
        filter: {
          levels: ["LOG_LEVEL_WARN", "LOG_LEVEL_ERROR"],
          targets: ["cadmus", "cadmus_core::device"],
          text: "",
        },
      });
    },
  );
});

test("summary streams cumulative snapshots", async () => {
  const response = streamResponse([
    connectFrame(
      0,
      JSON.stringify({ lines: "10", scanned: "100", size: "100" }),
    ),
    connectFrame(
      0,
      JSON.stringify({
        targets: [{ target: "cadmus::app", lines: "7" }],
        levels: [{ level: "LOG_LEVEL_WARN", lines: "3" }],
        lines: "10",
        firstTimestamp: "2025-01-01T00:00:00Z",
        lastTimestamp: "2025-01-01T00:00:09Z",
        scanned: "100",
        size: "100",
        done: true,
      }),
    ),
    connectFrame(2, "{}"),
  ]);
  const snapshots: LogSummary[] = [];
  await withStubbedFetch(
    () => response,
    async () => {
      await summarizeLog("1234", "run-1", (summary) => snapshots.push(summary));
    },
  );
  assert.deepEqual(snapshots, [
    {
      targets: [],
      levels: [],
      lines: 10,
      firstTimestamp: "",
      lastTimestamp: "",
      scanned: 100,
      size: 100,
      done: false,
    },
    {
      targets: [{ target: "cadmus::app", lines: 7 }],
      levels: [{ level: "warn", lines: 3 }],
      lines: 10,
      firstTimestamp: "2025-01-01T00:00:00Z",
      lastTimestamp: "2025-01-01T00:00:09Z",
      scanned: 100,
      size: 100,
      done: true,
    },
  ]);
});

test("summary rejects unknown log levels", async () => {
  const response = streamResponse([
    connectFrame(
      0,
      JSON.stringify({ levels: [{ level: "LOG_LEVEL_FATAL", lines: "1" }] }),
    ),
    connectFrame(2, "{}"),
  ]);
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(
        summarizeLog("1234", "run-1", () => {}),
        /LOG_LEVEL_FATAL/,
      );
    },
  );
});

test("download sends the ids and keeps the zip file name", async () => {
  const response = streamResponse([
    connectFrame(
      0,
      JSON.stringify({
        read: "3",
        total: "6",
        data: btoa("abc"),
        fileName: "cadmus-logs-20250101T000000Z.zip",
      }),
    ),
    connectFrame(
      0,
      JSON.stringify({ read: "6", total: "6", data: btoa("def") }),
    ),
    connectFrame(0, JSON.stringify({ read: "6", total: "6" })),
    connectFrame(2, "{}"),
  ]);
  const events: LogDownloadEvent[] = [];
  await withStubbedFetch(
    () => response,
    async (calls) => {
      await downloadLogs("1234", ["run-1"], (event) => events.push(event));
      const [call] = calls;
      assert.ok(call);
      assert.equal(call.url, "/cadmus.api.public.v1.Logs/Download");
      const request = call.init.body as Uint8Array;
      const length = new DataView(request.buffer).getUint32(1);
      assert.deepEqual(
        JSON.parse(new TextDecoder().decode(request.subarray(5, 5 + length))),
        { logIds: ["run-1"] },
      );
    },
  );
  assert.deepEqual(
    events.map((event) => [
      event.read,
      event.total,
      new TextDecoder().decode(event.data),
      event.fileName,
    ]),
    [
      [3, 6, "abc", "cadmus-logs-20250101T000000Z.zip"],
      [6, 6, "def", ""],
      [6, 6, "", ""],
    ],
  );
});

test("summarize sends one framed request to the Summarize method", async () => {
  const response = streamResponse([connectFrame(2, "{}")]);
  await withStubbedFetch(
    () => response,
    async (calls) => {
      await summarizeLog("1234", "run-1", () => {});
      const [call] = calls;
      assert.ok(call);
      assert.equal(call.url, "/cadmus.api.public.v1.Logs/Summarize");
      const request = call.init.body as Uint8Array;
      const length = new DataView(request.buffer).getUint32(1);
      assert.deepEqual(
        JSON.parse(new TextDecoder().decode(request.subarray(5, 5 + length))),
        { logId: "run-1" },
      );
    },
  );
});

function chunkedResponse(chunks: Uint8Array[]): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
  return new Response(stream, {
    status: 200,
    headers: { "content-type": "application/connect+json" },
  });
}

function splitBytes(bytes: Uint8Array, cuts: number[]): Uint8Array[] {
  const parts: Uint8Array[] = [];
  let start = 0;
  for (const cut of cuts) {
    parts.push(bytes.slice(start, cut));
    start = cut;
  }
  parts.push(bytes.slice(start));
  return parts;
}

test("frames split across chunks are reassembled", async () => {
  const first = connectFrame(0, JSON.stringify({ lines: "3", scanned: "5" }));
  const end = connectFrame(2, "{}");
  const bytes = new Uint8Array(first.length + end.length);
  bytes.set(first);
  bytes.set(end, first.length);
  const response = chunkedResponse(
    splitBytes(bytes, [2, 5, first.length - 3, first.length + 1]),
  );
  const snapshots: LogSummary[] = [];
  await withStubbedFetch(
    () => response,
    async () => {
      await summarizeLog("1234", "run-1", (summary) => snapshots.push(summary));
    },
  );
  assert.deepEqual(
    snapshots.map((summary) => [summary.lines, summary.scanned]),
    [[3, 5]],
  );
});

test("an unauthenticated end-of-stream frame is unauthorized", async () => {
  const response = streamResponse([
    connectFrame(
      2,
      JSON.stringify({
        error: { code: "unauthenticated", message: "wrong code" },
      }),
    ),
  ]);
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(
        summarizeLog("1234", "run-1", () => {}),
        (error: unknown) =>
          isUnauthorized(error) &&
          companionErrorCode(error) === "unauthenticated",
      );
    },
  );
});

test("a stream with the wrong content type fails", async () => {
  const response = new Response(JSON.stringify({ done: true }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(
        downloadLogs("1234", ["run-1"], () => {}),
        /content type/,
      );
    },
  );
});

test("a parse failure cancels the stream reader", async () => {
  let cancelled = false;
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(
        connectFrame(
          0,
          JSON.stringify({
            levels: [{ level: "LOG_LEVEL_FATAL", lines: "1" }],
          }),
        ),
      );
    },
    cancel() {
      cancelled = true;
    },
  });
  const response = new Response(stream, {
    status: 200,
    headers: { "content-type": "application/connect+json" },
  });
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(
        summarizeLog("1234", "run-1", () => {}),
        /LOG_LEVEL_FATAL/,
      );
    },
  );
  assert.equal(cancelled, true);
});

test("busy stream errors keep the resource_exhausted code", async () => {
  const response = streamResponse([
    connectFrame(
      2,
      JSON.stringify({
        error: {
          code: "resource_exhausted",
          message: "another download is running",
        },
      }),
    ),
  ]);
  await withStubbedFetch(
    () => response,
    async () => {
      await assert.rejects(
        downloadLogs("1234", ["run-1"], () => {}),
        (error: unknown) => companionErrorCode(error) === "resource_exhausted",
      );
    },
  );
});

test("unary errors keep the Connect code and message", async () => {
  await withStubbedFetch(
    () =>
      new Response(
        JSON.stringify({ code: "resource_exhausted", message: "busy" }),
        { status: 429, headers: { "content-type": "application/json" } },
      ),
    async () => {
      await assert.rejects(
        listLogs("1234"),
        (error: unknown) =>
          companionErrorCode(error) === "resource_exhausted" &&
          error instanceof Error &&
          error.message === "busy",
      );
    },
  );
});
