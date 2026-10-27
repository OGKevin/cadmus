export type SettingChoice = {
  selected: string;
  options: { id: string; label: string }[];
};

export type RefreshRateByKind = {
  extension: string;
  regular: number;
  inverted: number;
};

export type RefreshRateValue = {
  regular: number;
  inverted: number;
  byKind: RefreshRateByKind[];
  available: string[];
};

export type CheckItem = {
  id: string;
  label: string;
  on: boolean;
};

export type ChecksValue = {
  items: CheckItem[];
};

export type PathValue = {
  path: string;
};

export type LibraryValue = {
  name: string;
  path: PathValue;
  finished: SettingChoice;
  remove?: boolean;
};

export type SettingInput =
  boolean | string | RefreshRateValue | ChecksValue | PathValue | LibraryValue;

export type TextMode =
  "text" | "integer" | "number" | "hostname" | "coordinates";
export type ProtoSettingId = {
  kind: string;
  index?: number;
  key?: string;
};

export type Setting = {
  id: string;
  protoId: ProtoSettingId;
  label: string;
} & (
  | { kind: "toggle"; toggle: boolean }
  | { kind: "choice"; choice: SettingChoice }
  | { kind: "text"; mode: TextMode; text: string }
  | { kind: "refreshRate"; refreshRate: RefreshRateValue }
  | { kind: "checks"; items: CheckItem[] }
  | { kind: "path"; path: string }
  | { kind: "library"; library: LibraryValue }
);

export type Category = {
  id: string;
  label: string;
  settings: Setting[];
};

export type DirectoryEntry = {
  name: string;
  path: string;
  directory: boolean;
};

export type DirectoryListing = {
  path: string;
  parent: string | null;
  entries: DirectoryEntry[];
};

const HIDDEN_SETTING_IDS = new Set(["ForceFullImport"]);
const SETTING_KIND_PREFIX = "SETTING_IDENTITY_KIND_";
const CATEGORY_ID_PREFIX = "CATEGORY_ID_";

const listUrl = "/cadmus.api.shared.v1.Settings/List";
const updateUrl = "/cadmus.api.shared.v1.Settings/Update";
const addLibraryUrl = "/cadmus.api.shared.v1.Settings/AddLibrary";
const removeLibraryUrl = "/cadmus.api.shared.v1.Settings/RemoveLibrary";
const statusUrl = "/cadmus.api.shared.v1.Status/GetStatus";
const listDirectoryUrl = "/cadmus.api.shared.v1.FileOps/ListDirectory";
const getFileUrl = "/cadmus.api.shared.v1.FileOps/GetFile";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const KNOWN_SETTING_KINDS: Record<string, true> = {
  SETTING_IDENTITY_KIND_KEYBOARD_LAYOUT: true,
  SETTING_IDENTITY_KIND_LOCALE: true,
  SETTING_IDENTITY_KIND_AUTO_SUSPEND: true,
  SETTING_IDENTITY_KIND_WIFI_IDLE_TIMEOUT: true,
  SETTING_IDENTITY_KIND_AUTO_POWER_OFF: true,
  SETTING_IDENTITY_KIND_SLEEP_COVER: true,
  SETTING_IDENTITY_KIND_AUTO_SHARE: true,
  SETTING_IDENTITY_KIND_AUTO_TIME: true,
  SETTING_IDENTITY_KIND_NTP_SERVER: true,
  SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT: true,
  SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT_BRIGHTNESS: true,
  SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT_MANUAL_COORDINATES: true,
  SETTING_IDENTITY_KIND_BUTTON_SCHEME: true,
  SETTING_IDENTITY_KIND_LOGGING_ENABLED: true,
  SETTING_IDENTITY_KIND_FONT_FAMILY: true,
  SETTING_IDENTITY_KIND_FINISHED_ACTION: true,
  SETTING_IDENTITY_KIND_LIBRARY_INFO: true,
  SETTING_IDENTITY_KIND_LIBRARY_NAME: true,
  SETTING_IDENTITY_KIND_LIBRARY_PATH: true,
  SETTING_IDENTITY_KIND_LIBRARY_FINISHED_ACTION: true,
  SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND: true,
  SETTING_IDENTITY_KIND_INTERMISSION_POWER_OFF: true,
  SETTING_IDENTITY_KIND_INTERMISSION_SHARE: true,
  SETTING_IDENTITY_KIND_INTERMISSION_FILL_COLOR: true,
  SETTING_IDENTITY_KIND_ENABLE_THIRD_PARTY_LOG: true,
  SETTING_IDENTITY_KIND_SETTINGS_RETENTION: true,
  SETTING_IDENTITY_KIND_DB_BACKUP_RETENTION: true,
  SETTING_IDENTITY_KIND_LOG_LEVEL: true,
  SETTING_IDENTITY_KIND_IMPORT_SYNC_METADATA: true,
  SETTING_IDENTITY_KIND_METADATA_KINDS: true,
  SETTING_IDENTITY_KIND_ALLOWED_KINDS: true,
  SETTING_IDENTITY_KIND_FORCE_FULL_IMPORT: true,
  SETTING_IDENTITY_KIND_OTLP_ENDPOINT: true,
  SETTING_IDENTITY_KIND_PYROSCOPE_ENDPOINT: true,
  SETTING_IDENTITY_KIND_ENABLE_KERN_LOG: true,
  SETTING_IDENTITY_KIND_ENABLE_DBUS_LOG: true,
  SETTING_IDENTITY_KIND_DICTIONARY_INFO: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE_REGULAR: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE_INVERTED: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE_BY_KIND: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE_BY_KIND_REGULAR: true,
  SETTING_IDENTITY_KIND_REFRESH_RATE_BY_KIND_INVERTED: true,
  SETTING_IDENTITY_KIND_DITHERED_KINDS: true,
  SETTING_IDENTITY_KIND_STARTUP_MODE: true,
  SETTING_IDENTITY_KIND_AUTOSLEEP_MODE: true,
  SETTING_IDENTITY_KIND_INDICATE_AUTOSLEEP_LED: true,
  SETTING_IDENTITY_KIND_AUTOSLEEP_GRACE: true,
};

function hiddenSetting(id: string): boolean {
  return HIDDEN_SETTING_IDS.has(id);
}

function parseOptions(value: unknown): SettingChoice["options"] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!isRecord(item)) return [];
    if (typeof item.id !== "string" || typeof item.label !== "string") {
      return [];
    }
    return [{ id: item.id, label: item.label }];
  });
}

function parseChoiceValue(value: unknown): SettingChoice {
  if (!isRecord(value)) {
    return { selected: "", options: [] };
  }
  return {
    selected: typeof value.selected === "string" ? value.selected : "",
    options: parseOptions(value.options),
  };
}

function parseCheckItems(value: unknown): CheckItem[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!isRecord(item)) return [];
    if (typeof item.id !== "string" || typeof item.label !== "string") {
      return [];
    }
    return [{ id: item.id, label: item.label, on: item.on === true }];
  });
}

function parsePathString(value: unknown): string | null {
  if (typeof value === "string") return value;
  if (isRecord(value) && typeof value.path === "string") return value.path;
  return null;
}

function parseLibrary(value: unknown): LibraryValue | null {
  if (!isRecord(value)) return null;
  const path = parsePathString(value.path);
  if (path === null) return null;
  return {
    name: typeof value.name === "string" ? value.name : "",
    path: { path },
    finished: parseChoiceValue(value.finished),
  };
}

function parseSettingId(value: unknown): {
  id: string;
  protoId: ProtoSettingId;
} | null {
  const expected =
    'a SettingId message {"kind": "SETTING_IDENTITY_KIND_...", "index"?, "key"?}';
  if (!isRecord(value) || typeof value.kind !== "string") {
    throw new Error(
      `Companion settings row id is ${JSON.stringify(value)}; expected ${expected}`,
    );
  }
  const suffix = value.kind.slice(SETTING_KIND_PREFIX.length);
  if (
    !value.kind.startsWith(SETTING_KIND_PREFIX) ||
    !/^[A-Z][A-Z0-9_]*$/.test(suffix)
  ) {
    throw new Error(
      `Companion settings row id kind ${JSON.stringify(value.kind)} is not a SettingIdentityKind name; expected ${expected}`,
    );
  }
  const protoId: ProtoSettingId = { kind: value.kind };
  if (value.index !== undefined) {
    if (typeof value.index !== "number" || !Number.isInteger(value.index)) {
      throw new Error(
        `Companion settings row id is ${JSON.stringify(value)}; expected ${expected}`,
      );
    }
    protoId.index = value.index;
  }
  if (value.key !== undefined) {
    if (typeof value.key !== "string") {
      throw new Error(
        `Companion settings row id is ${JSON.stringify(value)}; expected ${expected}`,
      );
    }
    protoId.key = value.key;
  }
  if (!KNOWN_SETTING_KINDS[value.kind]) {
    console.warn(
      `Companion settings row kind ${value.kind} is not known to the frontend; skipping it. Add it to KNOWN_SETTING_KINDS in lib/companion.ts to render it.`,
    );
    return null;
  }
  const parts = [
    suffix
      .split("_")
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1).toLowerCase())
      .join(""),
  ];
  if (protoId.index !== undefined) parts.push(String(protoId.index));
  if (protoId.key !== undefined) parts.push(protoId.key);
  return { id: parts.join(":"), protoId };
}

function parseCategoryId(value: unknown): string {
  if (
    typeof value !== "string" ||
    !value.startsWith(CATEGORY_ID_PREFIX) ||
    !/^[A-Z][A-Z0-9_]*$/.test(value.slice(CATEGORY_ID_PREFIX.length))
  ) {
    throw new Error(
      `Companion settings category id is ${JSON.stringify(value)}; expected a CategoryId name like "CATEGORY_ID_LIBRARIES"`,
    );
  }
  return value.slice(CATEGORY_ID_PREFIX.length).toLowerCase();
}

function parseSetting(value: unknown): Setting | null {
  if (!isRecord(value)) {
    throw new Error(
      `Companion settings row is ${JSON.stringify(value)}; expected a Setting message {"id", "label", "value"}`,
    );
  }
  const parsedId = parseSettingId(value.id);
  if (!parsedId) return null;
  if (hiddenSetting(parsedId.id)) return null;

  const base = {
    id: parsedId.id,
    protoId: parsedId.protoId,
    label: typeof value.label === "string" ? value.label : "",
  };
  const payload = isRecord(value.value) ? value.value : value;

  if (typeof payload.toggle === "boolean") {
    return { ...base, kind: "toggle", toggle: payload.toggle };
  }

  if (isRecord(payload.choice)) {
    return {
      ...base,
      kind: "choice",
      choice: parseChoiceValue(payload.choice),
    };
  }

  const refresh = payload.refreshRate ?? payload.refresh_rate;
  if (refresh !== undefined) {
    const parsed = parseRefreshRate(refresh);
    if (!parsed) return null;
    return { ...base, kind: "refreshRate", refreshRate: parsed };
  }

  if (payload.checks !== undefined) {
    const checks = payload.checks;
    const items = isRecord(checks)
      ? parseCheckItems(checks.items)
      : parseCheckItems(checks);
    return { ...base, kind: "checks", items };
  }

  if (payload.library !== undefined) {
    const library = parseLibrary(payload.library);
    if (!library) return null;
    return { ...base, kind: "library", library };
  }

  if (payload.path !== undefined) {
    const path = parsePathString(payload.path);
    if (path === null) return null;
    return { ...base, kind: "path", path };
  }

  if (isRecord(payload.text)) {
    return {
      ...base,
      kind: "text",
      mode: parseTextMode(payload.text.mode),
      text: typeof payload.text.text === "string" ? payload.text.text : "",
    };
  }

  if (typeof payload.text === "string") {
    return { ...base, kind: "text", mode: "text", text: payload.text };
  }

  console.warn(
    `Companion settings row ${base.id} has no recognized value; expected one of toggle, text, choice, refreshRate, checks, path, library`,
  );
  return null;
}

function parseSettings(value: unknown): Category[] {
  if (!isRecord(value)) {
    throw new Error("Companion settings response is invalid");
  }
  if (value.categories === undefined) return [];
  if (!Array.isArray(value.categories)) {
    throw new Error("Companion settings response is invalid");
  }

  return value.categories.flatMap((category) => {
    if (!isRecord(category)) {
      throw new Error(
        `Companion settings category is ${JSON.stringify(category)}; expected a SettingsCategory message {"id", "label", "settings"}`,
      );
    }
    const settings = Array.isArray(category.settings)
      ? category.settings.flatMap((setting) => {
          const parsed = parseSetting(setting);
          return parsed ? [parsed] : [];
        })
      : [];
    return [
      {
        id: parseCategoryId(category.id),
        label: typeof category.label === "string" ? category.label : "",
        settings,
      },
    ];
  });
}

function parseTextMode(value: unknown): TextMode {
  if (value === 2 || value === "TEXT_MODE_INTEGER") return "integer";
  if (value === 3 || value === "TEXT_MODE_NUMBER") return "number";
  if (value === 4 || value === "TEXT_MODE_HOSTNAME") return "hostname";
  if (value === 5 || value === "TEXT_MODE_COORDINATES") return "coordinates";
  return "text";
}

function parseRate(value: unknown): number | null {
  if (value === undefined) return 0;
  if (
    typeof value === "number" &&
    Number.isInteger(value) &&
    value >= 0 &&
    value <= 255
  ) {
    return value;
  }
  return null;
}

function parseRefreshRate(value: unknown): RefreshRateValue | null {
  if (!isRecord(value)) return null;
  const regular = parseRate(value.regular);
  const inverted = parseRate(value.inverted);
  if (regular === null || inverted === null) return null;
  const byKindRaw = value.byKind ?? value.by_kind;
  const byKind = Array.isArray(byKindRaw)
    ? byKindRaw.flatMap((item) => {
        if (!isRecord(item) || typeof item.extension !== "string") return [];
        const itemRegular = parseRate(item.regular);
        const itemInverted = parseRate(item.inverted);
        if (itemRegular === null || itemInverted === null) return [];
        return [
          {
            extension: item.extension,
            regular: itemRegular,
            inverted: itemInverted,
          },
        ];
      })
    : [];
  const available = Array.isArray(value.available)
    ? value.available.flatMap((item) =>
        typeof item === "string" ? [item] : [],
      )
    : [];
  return { regular, inverted, byKind, available };
}

function parseDirectoryListing(value: unknown): DirectoryListing {
  if (!isRecord(value)) {
    throw new Error("Companion directory response is invalid");
  }
  const path = typeof value.path === "string" ? value.path : "";
  const parent =
    typeof value.parent === "string" && value.parent !== ""
      ? value.parent
      : null;
  const entries = Array.isArray(value.entries)
    ? value.entries.flatMap((item) => {
        if (!isRecord(item)) return [];
        if (typeof item.name !== "string" || typeof item.path !== "string") {
          return [];
        }
        return [
          {
            name: item.name,
            path: item.path,
            directory: item.directory === true,
          },
        ];
      })
    : [];
  return { path, parent, entries };
}

function decodeBase64(value: string): ArrayBuffer {
  try {
    const binary = atob(value);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
  } catch {
    throw new Error("Companion file response is invalid");
  }
}

const CONNECT_STREAM_TYPE = "application/connect+json";

function connectFrame(payload: string): Uint8Array<ArrayBuffer> {
  const bytes = new TextEncoder().encode(payload);
  const frame = new Uint8Array(5 + bytes.length);
  new DataView(frame.buffer).setUint32(1, bytes.length);
  frame.set(bytes, 5);
  return frame;
}

function mergeBytes(head: Uint8Array, tail: Uint8Array): Uint8Array {
  const merged = new Uint8Array(head.length + tail.length);
  merged.set(head);
  merged.set(tail, head.length);
  return merged;
}

function parseFrameJson(payload: Uint8Array): unknown {
  try {
    return JSON.parse(new TextDecoder().decode(payload)) as unknown;
  } catch {
    throw new Error(
      "Companion file response frame is not JSON; expected application/connect+json frames",
    );
  }
}

function connectStreamError(error: Record<string, unknown>): Error {
  const code = typeof error.code === "string" ? error.code : "unknown";
  const message = typeof error.message === "string" ? error.message : code;
  return Object.assign(
    new Error(`Companion request failed: ${code}: ${message}`),
    { status: code === "unauthenticated" ? 401 : 500 },
  );
}

async function readConnectMessages(
  response: Response,
  onMessage: (message: unknown) => void,
): Promise<void> {
  const body = response.body;
  if (!body) {
    throw new Error("Companion file response is empty");
  }
  const reader = body.getReader();
  let buffer: Uint8Array<ArrayBufferLike> = new Uint8Array(0);
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer = buffer.length === 0 ? value : mergeBytes(buffer, value);
    let offset = 0;
    while (buffer.length - offset >= 5) {
      const flags = buffer[offset];
      const length =
        buffer[offset + 1] * 2 ** 24 +
        buffer[offset + 2] * 2 ** 16 +
        buffer[offset + 3] * 2 ** 8 +
        buffer[offset + 4];
      if (buffer.length - offset < 5 + length) break;
      const payload = buffer.subarray(offset + 5, offset + 5 + length);
      offset += 5 + length;
      if (flags === 0x02) {
        const message = parseFrameJson(payload);
        const error = isRecord(message) ? message.error : undefined;
        if (isRecord(error)) throw connectStreamError(error);
        return;
      }
      if (flags !== 0x00) {
        throw new Error(
          `Companion file response frame flag ${flags} is not a JSON message frame; expected 0 or 2`,
        );
      }
      onMessage(parseFrameJson(payload));
    }
    buffer =
      offset === 0
        ? buffer
        : offset === buffer.length
          ? new Uint8Array(0)
          : buffer.slice(offset);
  }
  throw new Error(
    "Companion file response ended without an end-of-stream frame; expected application/connect+json frames",
  );
}

async function parseFileChunks(response: Response): Promise<Blob> {
  const parts: ArrayBuffer[] = [];
  let contentType = "";
  await readConnectMessages(response, (message) => {
    if (
      !isRecord(message) ||
      (message.data !== undefined && typeof message.data !== "string")
    ) {
      throw new Error(
        `Companion file response frame is ${JSON.stringify(message)}; expected a FileChunk message {"data"?, "contentType"?}`,
      );
    }
    if (!contentType && typeof message.contentType === "string") {
      contentType = message.contentType;
    }
    parts.push(
      decodeBase64(typeof message.data === "string" ? message.data : ""),
    );
  });
  return new Blob(parts, { type: contentType || "application/octet-stream" });
}

function isRefreshRate(value: SettingInput): value is RefreshRateValue {
  return (
    typeof value === "object" &&
    "regular" in value &&
    "inverted" in value &&
    "byKind" in value
  );
}

function isChecksValue(value: SettingInput): value is ChecksValue {
  return typeof value === "object" && "items" in value;
}

function isPathValue(value: SettingInput): value is PathValue {
  return (
    typeof value === "object" &&
    "path" in value &&
    typeof value.path === "string"
  );
}

function isLibraryValue(value: SettingInput): value is LibraryValue {
  return (
    typeof value === "object" &&
    "name" in value &&
    "path" in value &&
    typeof value.path === "object" &&
    "finished" in value
  );
}

export function isLibraryRemove(value: SettingInput): boolean {
  return isLibraryValue(value) && value.remove === true;
}

const FILL_COLOR_WHITE = "#ffffff";
const FILL_COLOR_BLACK = "#000000";

export function fillColorChoice(
  choice: SettingChoice,
  selected = choice.selected,
): SettingChoice {
  const options = choice.options.filter(
    (option) =>
      option.id === FILL_COLOR_WHITE || option.id === FILL_COLOR_BLACK,
  );
  if (
    selected !== FILL_COLOR_WHITE &&
    selected !== FILL_COLOR_BLACK &&
    selected !== ""
  ) {
    options.push({ id: selected, label: selected });
  }
  return { selected, options };
}

export function withSettingValue(
  setting: Setting,
  value: SettingInput,
): Setting {
  if (setting.kind === "toggle" && typeof value === "boolean") {
    return { ...setting, toggle: value };
  }
  if (setting.kind === "choice" && typeof value === "string") {
    if (setting.id === "IntermissionFillColor") {
      return { ...setting, choice: fillColorChoice(setting.choice, value) };
    }
    const options = setting.choice.options.some((option) => option.id === value)
      ? setting.choice.options
      : [...setting.choice.options, { id: value, label: value }];
    return { ...setting, choice: { selected: value, options } };
  }
  if (setting.kind === "text" && typeof value === "string") {
    return { ...setting, text: value };
  }
  if (setting.kind === "refreshRate" && isRefreshRate(value)) {
    return { ...setting, refreshRate: value };
  }
  if (setting.kind === "checks" && isChecksValue(value)) {
    return { ...setting, items: value.items };
  }
  if (setting.kind === "path" && isPathValue(value)) {
    return { ...setting, path: value.path };
  }
  if (setting.kind === "library" && isLibraryValue(value)) {
    return {
      ...setting,
      label: value.name || setting.label,
      protoId: { ...setting.protoId, key: value.path.path },
      library: value,
    };
  }
  return setting;
}

export function applySetting(
  categories: Category[],
  id: string,
  value: SettingInput,
): Category[] {
  if (isLibraryRemove(value)) {
    return categories.map((category) => ({
      ...category,
      settings: category.settings.filter((setting) => setting.id !== id),
    }));
  }
  return categories.map((category) => ({
    ...category,
    settings: category.settings.map((setting) =>
      setting.id === id ? withSettingValue(setting, value) : setting,
    ),
  }));
}

export function takeQueuedUpdate(
  queue: Map<string, SettingInput>,
  categories: Category[] | null,
): [string, SettingInput] | undefined {
  const known = new Set(
    (categories ?? []).flatMap((category) =>
      category.settings.map((setting) => setting.id),
    ),
  );
  for (const [id, value] of queue) {
    queue.delete(id);
    if (known.has(id)) return [id, value];
  }
  return undefined;
}

function updateBody(setting: Setting, value: SettingInput) {
  if (setting.kind === "toggle" && typeof value === "boolean") {
    return { id: setting.protoId, value: { toggle: value } };
  }
  if (setting.kind === "choice" && typeof value === "string") {
    return { id: setting.protoId, value: { choice: { selected: value } } };
  }
  if (setting.kind === "text" && typeof value === "string") {
    return {
      id: setting.protoId,
      value: {
        text: { text: value, mode: TEXT_MODE_NAMES[setting.mode] },
      },
    };
  }
  if (setting.kind === "refreshRate" && isRefreshRate(value)) {
    return {
      id: setting.protoId,
      value: {
        refreshRate: {
          regular: value.regular,
          inverted: value.inverted,
          byKind: value.byKind.map((entry) => ({
            extension: entry.extension,
            regular: entry.regular,
            inverted: entry.inverted,
          })),
        },
      },
    };
  }
  if (setting.kind === "checks" && isChecksValue(value)) {
    return {
      id: setting.protoId,
      value: { checks: { items: value.items } },
    };
  }
  if (setting.kind === "path" && isPathValue(value)) {
    return { id: setting.protoId, value: { path: { path: value.path } } };
  }
  if (setting.kind === "library" && isLibraryValue(value)) {
    return {
      id: setting.protoId,
      value: {
        library: {
          name: value.name,
          path: { path: value.path.path },
          finished: { selected: value.finished.selected },
        },
      },
    };
  }
  throw new Error("Companion setting value does not match its kind");
}

const TEXT_MODE_NAMES: Record<TextMode, string> = {
  text: "TEXT_MODE_TEXT",
  integer: "TEXT_MODE_INTEGER",
  number: "TEXT_MODE_NUMBER",
  hostname: "TEXT_MODE_HOSTNAME",
  coordinates: "TEXT_MODE_COORDINATES",
};

function libraryBody(value: LibraryValue) {
  return {
    library: {
      name: value.name,
      path: { path: value.path.path },
      finished: { selected: value.finished.selected },
    },
  };
}

async function requestError(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => null);
  const message =
    isRecord(body) && typeof body.message === "string" && body.message
      ? body.message
      : "Companion request failed";
  return Object.assign(new Error(message), { status: response.status });
}

async function postJson(
  token: string,
  url: string,
  body: unknown,
): Promise<unknown> {
  const response = await fetch(url, {
    method: "POST",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      "Connect-Protocol-Version": "1",
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await requestError(response);
  const text = await response.text();
  if (!text) return {};
  return JSON.parse(text) as unknown;
}

export function isUnauthorized(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "status" in error &&
    (error as { status: unknown }).status === 401
  );
}

export type CompanionFailure =
  { kind: "unauthorized" } | { kind: "visible"; message: string };

export function classifyCompanionFailure(error: unknown): CompanionFailure {
  if (isUnauthorized(error)) return { kind: "unauthorized" };
  return {
    kind: "visible",
    message: error instanceof Error ? error.message : String(error),
  };
}

/** Removing a library renumbers the server's rows, so the view must reload. */
export function shouldReloadAfterLibraryChange(
  change: "remove" | "update",
): boolean {
  return change === "remove";
}

export async function listSettings(token: string): Promise<Category[]> {
  return parseSettings(await postJson(token, listUrl, {}));
}

export async function listDirectory(
  token: string,
  path: string,
  extensions?: string[],
): Promise<DirectoryListing> {
  const body =
    extensions && extensions.length > 0 ? { path, extensions } : { path };
  return parseDirectoryListing(await postJson(token, listDirectoryUrl, body));
}

export async function getFile(token: string, path: string): Promise<Blob> {
  const response = await fetch(getFileUrl, {
    method: "POST",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": CONNECT_STREAM_TYPE,
      "Connect-Protocol-Version": "1",
    },
    body: connectFrame(JSON.stringify({ path })),
  });
  if (!response.ok) throw await requestError(response);
  const type = response.headers.get("content-type") ?? "";
  if (!type.startsWith(CONNECT_STREAM_TYPE)) {
    throw new Error(
      `Companion file response content type is ${type}; expected ${CONNECT_STREAM_TYPE} frames`,
    );
  }
  return parseFileChunks(response);
}

export async function addLibrary(
  token: string,
  value: LibraryValue,
): Promise<void> {
  await postJson(token, addLibraryUrl, libraryBody(value));
}

export async function removeLibrary(
  token: string,
  setting: Setting,
): Promise<void> {
  await postJson(token, removeLibraryUrl, { id: setting.protoId });
}

export type CompanionStatus = {
  version: string;
  hostname: string;
  locale: string;
};

export type CompanionProbe =
  ({ state: "up" } & CompanionStatus) | { state: "down" } | { state: "absent" };

function statusText(body: Record<string, unknown>, key: string) {
  const value = body[key];
  return typeof value === "string" ? value : "";
}

function statusNumber(record: Record<string, unknown>, key: string) {
  const value = record[key];
  if (typeof value === "number") return value;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

function versionText(body: Record<string, unknown>) {
  const version = body.version;
  if (!isRecord(version)) return "";
  const numbers = ["major", "minor", "patch"].map((key) =>
    statusNumber(version, key),
  );
  const ahead = statusNumber(version, "commitsAhead");
  const suffix = ahead > 0 ? `+${ahead}` : "";
  const dirty = version.dirty === true ? "-dirty" : "";
  return `v${numbers.join(".")}${suffix}${dirty}`;
}

function localeText(body: Record<string, unknown>) {
  const locale = statusText(body, "locale");
  if (!locale.startsWith("LOCALE_")) return locale.toLowerCase();
  return locale.slice("LOCALE_".length).toLowerCase().replace("_", "-");
}

export async function probeCompanion(): Promise<CompanionProbe> {
  const response = await fetch(statusUrl, {
    method: "POST",
    cache: "no-store",
    referrerPolicy: "no-referrer",
    headers: {
      "Content-Type": "application/json",
      "Connect-Protocol-Version": "1",
    },
    body: "{}",
  });
  const body: unknown = await response.json().catch(() => null);
  if (response.status === 503 && isRecord(body) && body.companion === "down") {
    return { state: "down" };
  }
  const version = isRecord(body) ? versionText(body) : "";
  if (!response.ok || !isRecord(body) || !version) {
    return { state: "absent" };
  }
  return {
    state: "up",
    version,
    hostname: statusText(body, "hostname"),
    locale: localeText(body),
  };
}

export async function updateSetting(
  token: string,
  setting: Setting,
  value: SettingInput,
): Promise<void> {
  await postJson(token, updateUrl, updateBody(setting, value));
}
