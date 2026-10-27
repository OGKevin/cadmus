import assert from "node:assert/strict";
import test from "node:test";
import {
  fillColorChoice,
  getFile,
  listSettings,
  updateSetting,
  withSettingValue,
  type Setting,
  type SettingChoice,
} from "./companion.ts";

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
