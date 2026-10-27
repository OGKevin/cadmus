"use client";

import {
  useEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type FormEvent,
  type KeyboardEvent,
} from "react";
import {
  CaretRightIcon,
  CheckIcon,
  FolderSimpleIcon,
  ImageIcon,
} from "@phosphor-icons/react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Empty } from "@cloudflare/kumo/components/empty";
import { Input } from "@cloudflare/kumo/components/input";
import { Loader } from "@cloudflare/kumo/components/loader";
import { Select } from "@cloudflare/kumo/components/select";
import { Switch } from "@cloudflare/kumo/components/switch";
import { Text } from "@cloudflare/kumo/components/text";
import { useTranslations } from "next-intl";
import {
  fillColorChoice,
  type CheckItem,
  type DirectoryEntry,
  type DirectoryListing,
  type LibraryValue,
  type RefreshRateValue,
  type Setting,
  type SettingChoice,
  type SettingInput,
  type TextMode,
} from "@/lib/companion";

const SAVE_DELAY_MS = 500;
const IMAGE_EXTENSIONS = [
  "EXTENSION_PNG",
  "EXTENSION_JPG",
  "EXTENSION_JPEG",
  "EXTENSION_WEBP",
  "EXTENSION_SVG",
];
const INTERMISSION_IMAGE_IDS = new Set([
  "IntermissionSuspend",
  "IntermissionPowerOff",
  "IntermissionShare",
]);

export function SettingRow({
  setting,
  pending = false,
  savedTick,
  onGetFile,
  onListDirectory,
  onUpdate,
}: {
  setting: Setting;
  pending?: boolean;
  savedTick?: number;
  onGetFile?: (path: string) => Promise<Blob>;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (value: SettingInput) => void;
}) {
  if (setting.kind === "refreshRate") {
    return (
      <RefreshRateSetting
        pending={pending}
        savedTick={savedTick}
        setting={setting}
        onUpdate={onUpdate}
      />
    );
  }

  if (setting.kind === "checks") {
    return (
      <ChecksSetting pending={pending} setting={setting} onUpdate={onUpdate} />
    );
  }

  if (setting.kind === "library") {
    return (
      <LibrarySetting
        pending={pending}
        savedTick={savedTick}
        setting={setting}
        onListDirectory={onListDirectory}
        onUpdate={onUpdate}
      />
    );
  }

  if (setting.kind === "path") {
    return (
      <PathSetting
        pending={pending}
        setting={setting}
        onListDirectory={onListDirectory}
        onUpdate={onUpdate}
      />
    );
  }

  return (
    <div className="flex flex-col gap-3 rounded-xl border border-kumo-line bg-kumo-base p-4 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex min-w-0 flex-col gap-1">
        <Text aria-hidden as="span">
          {setting.label}
        </Text>
      </div>
      <SettingControl
        pending={pending}
        savedTick={savedTick}
        setting={setting}
        onGetFile={onGetFile}
        onListDirectory={onListDirectory}
        onUpdate={onUpdate}
      />
    </div>
  );
}

function SettingControl({
  setting,
  pending,
  savedTick,
  onGetFile,
  onListDirectory,
  onUpdate,
}: {
  setting: Setting;
  pending: boolean;
  savedTick?: number;
  onGetFile?: (path: string) => Promise<Blob>;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (value: SettingInput) => void;
}) {
  const t = useTranslations("companion");
  const disabled = pending;

  if (setting.kind === "toggle") {
    return (
      <Switch
        aria-label={setting.label}
        checked={setting.toggle}
        disabled={disabled}
        onCheckedChange={onUpdate}
      />
    );
  }

  if (setting.kind === "choice") {
    if (setting.id === "IntermissionFillColor") {
      return (
        <FillColorControl
          choice={setting.choice}
          disabled={disabled}
          label={setting.label}
          placeholder={t("choose")}
          rgbLabel={t("rgb")}
          onUpdate={onUpdate}
        />
      );
    }
    if (INTERMISSION_IMAGE_IDS.has(setting.id)) {
      return (
        <ImageChoiceControl
          choice={setting.choice}
          disabled={disabled}
          label={setting.label}
          onGetFile={onGetFile}
          onListDirectory={onListDirectory}
          onUpdate={onUpdate}
        />
      );
    }
    return (
      <ChoiceSelect
        choice={setting.choice}
        disabled={disabled}
        label={setting.label}
        placeholder={t("choose")}
        onUpdate={onUpdate}
      />
    );
  }

  if (setting.kind !== "text") return null;

  return (
    <TextSetting
      label={setting.label}
      mode={setting.mode}
      savedLabel={t("saved")}
      savedTick={savedTick}
      value={setting.text}
      onUpdate={onUpdate}
    />
  );
}

function draftAllowed(mode: TextMode, value: string) {
  if (mode === "integer") return /^\d*$/.test(value);
  if (mode === "number") return /^-?\d*\.?\d*$/.test(value);
  if (mode === "hostname") return hostnameDraftAllowed(value);
  if (mode === "coordinates") return coordinateDraftAllowed(value);
  return true;
}

function draftReady(mode: TextMode, value: string) {
  if (mode === "integer") return /^\d+$/.test(value);
  if (mode === "number") return /^-?(?:\d+(?:\.\d+)?|\.\d+)$/.test(value);
  if (mode === "hostname") return hostnameDraftReady(value);
  if (mode === "coordinates") return coordinateDraftReady(value);
  return true;
}

function hostnameDraftAllowed(value: string) {
  if (value === "") return true;
  if (value.length > 253) return false;
  if (value.startsWith("[")) return bracketDraft(value);
  if (value.includes(":")) {
    return ipv6Draft(value) || namePortDraft(value);
  }
  return hostnameLabelsAllowed(value, true);
}

function hostnameDraftReady(value: string) {
  const trimmed = value.trim();
  if (trimmed === "") return true;
  return isNetworkAddress(trimmed);
}

function isNetworkAddress(value: string) {
  const host = stripNtpPort(value);
  return isIpv4(host) || isIpv6(host) || isHostname(host);
}

function stripNtpPort(value: string) {
  const bracket = value.match(/^\[(.+)]:123$/);
  if (bracket) return bracket[1];
  const index = value.lastIndexOf(":");
  if (index === -1 || value.slice(index + 1) !== "123") return value;
  const host = value.slice(0, index);
  if (!host.includes(":") || isIpv4(host) || isIpv6(host)) return host;
  return value;
}

function isHostname(value: string) {
  if (value.length === 0 || value.length > 253) return false;
  if (value.startsWith(".") || value.endsWith(".")) return false;
  return hostnameLabelsAllowed(value, false);
}

function hostnameLabelsAllowed(value: string, prefix: boolean) {
  if (!/^[A-Za-z0-9.-]*$/.test(value)) return false;
  if (value.includes("..") || value.startsWith(".")) return false;
  const labels = value.split(".");
  return labels.every((label, index) => {
    const last = index === labels.length - 1;
    if (label === "") return prefix && last;
    if (label.length > 63 || label.startsWith("-")) return false;
    if (label.endsWith("-") && !(prefix && last)) return false;
    return /^[A-Za-z0-9-]+$/.test(label);
  });
}

function namePortDraft(value: string) {
  const index = value.lastIndexOf(":");
  if (index === -1) return hostnameLabelsAllowed(value, true);
  const name = value.slice(0, index);
  const port = value.slice(index + 1);
  if (name === "" || name.includes(":")) return false;
  if (!hostnameLabelsAllowed(name, false)) return false;
  return "123".startsWith(port);
}

function bracketDraft(value: string) {
  if (!value.startsWith("[") || value.indexOf("[", 1) !== -1) return false;
  const end = value.indexOf("]");
  if (end === -1) return ipv6Draft(value.slice(1));
  if (value.indexOf("]", end + 1) !== -1) return false;
  const inside = value.slice(1, end);
  if (inside === "" || !ipv6Draft(inside)) return false;
  const rest = value.slice(end + 1);
  if (rest === "") return true;
  if (!rest.startsWith(":")) return false;
  return "123".startsWith(rest.slice(1));
}

function ipv6Draft(value: string) {
  if (value === "") return true;
  if (value.includes(".") || value.includes(":::")) return false;
  if (!/^[0-9A-Fa-f:]+$/.test(value)) return false;
  if (value.split("::").length - 1 > 1) return false;
  const groups = value.split(":").filter((group) => group.length > 0);
  if (groups.some((group) => group.length > 4)) return false;
  const max = value.includes("::") ? 7 : 8;
  return groups.length <= max;
}

function isIpv4(value: string) {
  const parts = value.split(".");
  if (parts.length !== 4) return false;
  return parts.every((part) => {
    if (!/^\d{1,3}$/.test(part)) return false;
    if (part.length > 1 && part.startsWith("0")) return false;
    const octet = Number(part);
    return octet <= 255;
  });
}

function isIpv6(value: string) {
  if (!ipv6Draft(value) || value === "" || value.includes(".")) return false;
  if (value.startsWith(":") && !value.startsWith("::")) return false;
  if (value.endsWith(":") && !value.endsWith("::")) return false;
  const groups = value.split(":").filter((group) => group.length > 0);
  if (value.includes("::")) return groups.length <= 7;
  return groups.length === 8;
}

function coordinateDraftAllowed(value: string) {
  if (!/^\s*-?\d*\.?\d*(,\s*-?\d*\.?\d*)?\s*$/.test(value)) return false;
  const comma = value.indexOf(",");
  const latitude = (comma === -1 ? value : value.slice(0, comma)).trim();
  if (comma !== -1 && latitude === "") return false;
  if (!partialInRange(latitude, -90, 90)) return false;
  if (comma === -1) return true;
  return partialInRange(value.slice(comma + 1), -180, 180);
}

function coordinateDraftReady(value: string) {
  const trimmed = value.trim();
  if (trimmed === "") return true;
  const match = trimmed.match(/^(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)$/);
  if (!match) return false;
  const latitude = Number(match[1]);
  const longitude = Number(match[2]);
  return (
    latitude >= -90 && latitude <= 90 && longitude >= -180 && longitude <= 180
  );
}

function partialInRange(raw: string, min: number, max: number) {
  const text = raw.trim();
  if (text === "" || text === "-" || text === "." || text === "-.") return true;
  if (!/^-?\d*\.?\d*$/.test(text)) return false;
  const number = Number(text.endsWith(".") ? text.slice(0, -1) : text);
  return Number.isFinite(number) && number >= min && number <= max;
}

function fieldMode(mode: TextMode) {
  if (mode === "integer") return "numeric" as const;
  if (mode === "number") return "decimal" as const;
  return "text" as const;
}

function inputAfterEdit(input: HTMLInputElement, insert: string) {
  const start = input.selectionStart ?? input.value.length;
  const end = input.selectionEnd ?? input.value.length;
  return input.value.slice(0, start) + insert + input.value.slice(end);
}

function blockDisallowedKey(
  event: KeyboardEvent<HTMLInputElement>,
  allowed: (value: string) => boolean,
) {
  if (
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    event.key.length !== 1
  ) {
    return;
  }
  if (!allowed(inputAfterEdit(event.currentTarget, event.key))) {
    event.preventDefault();
  }
}

function blockDisallowedBeforeInput(
  event: FormEvent<HTMLInputElement>,
  allowed: (value: string) => boolean,
) {
  const native = event.nativeEvent;
  if (!(native instanceof InputEvent)) return;
  if (
    native.inputType.startsWith("delete") ||
    native.inputType === "historyUndo" ||
    native.inputType === "historyRedo"
  ) {
    return;
  }
  if (!allowed(inputAfterEdit(event.currentTarget, native.data ?? ""))) {
    event.preventDefault();
  }
}

function blockDisallowedPaste(
  event: ClipboardEvent<HTMLInputElement>,
  allowed: (value: string) => boolean,
) {
  const pasted = event.clipboardData.getData("text");
  if (!allowed(inputAfterEdit(event.currentTarget, pasted))) {
    event.preventDefault();
  }
}

function rateAllowed(value: string) {
  if (value === "") return true;
  if (!/^\d+$/.test(value)) return false;
  return Number(value) <= 255;
}

function TextSetting({
  label,
  mode,
  disabled = false,
  savedLabel,
  savedTick,
  value,
  onUpdate,
}: {
  label: string;
  mode: TextMode;
  disabled?: boolean;
  savedLabel: string;
  savedTick?: number;
  value: string;
  onUpdate: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const [localTick, setLocalTick] = useState(0);
  const onUpdateRef = useRef(onUpdate);
  const valueRef = useRef(value);
  const controlled = useRef(savedTick !== undefined);
  const tick = savedTick ?? localTick;

  useEffect(() => {
    onUpdateRef.current = onUpdate;
  }, [onUpdate]);

  useEffect(() => {
    valueRef.current = value;
    controlled.current = savedTick !== undefined;
  }, [savedTick, value]);

  useEffect(() => {
    if (disabled || !draftReady(mode, draft) || draft === valueRef.current) {
      return;
    }
    const timer = window.setTimeout(() => {
      if (!draftReady(mode, draft) || draft === valueRef.current) return;
      onUpdateRef.current(draft);
      if (!controlled.current) setLocalTick((next) => next + 1);
    }, SAVE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [draft, mode, disabled]);

  return (
    <div className="flex w-full items-center gap-2 sm:w-auto">
      <Input
        aria-label={label}
        autoComplete="off"
        className="w-full sm:w-64"
        disabled={disabled}
        inputMode={fieldMode(mode)}
        passwordManagerIgnore
        value={draft}
        onBeforeInput={(event: FormEvent<HTMLInputElement>) => {
          if (mode !== "text")
            blockDisallowedBeforeInput(event, (next) =>
              draftAllowed(mode, next),
            );
        }}
        onBlur={() => {
          if (!draftReady(mode, draft)) setDraft(valueRef.current);
        }}
        onChange={(event) => {
          const next = event.target.value;
          if (!draftAllowed(mode, next)) {
            event.currentTarget.value = draft;
            return;
          }
          setDraft(next);
        }}
        onKeyDown={(event) => {
          if (mode !== "text") {
            blockDisallowedKey(event, (next) => draftAllowed(mode, next));
          }
        }}
        onPaste={(event) => {
          if (mode !== "text")
            blockDisallowedPaste(event, (next) => draftAllowed(mode, next));
        }}
      />
      <span aria-live="polite" className="inline-flex w-5 justify-center">
        <SavedMark label={savedLabel} tick={tick} />
      </span>
    </div>
  );
}

function refreshKey(value: RefreshRateValue) {
  return JSON.stringify({
    regular: value.regular,
    inverted: value.inverted,
    byKind: value.byKind,
  });
}

function extensionLabel(extension: string) {
  return extension.replace(/^EXTENSION_/, "").toUpperCase();
}

function RefreshRateSetting({
  setting,
  pending,
  savedTick,
  onUpdate,
}: {
  setting: Extract<Setting, { kind: "refreshRate" }>;
  pending: boolean;
  savedTick?: number;
  onUpdate: (value: RefreshRateValue) => void;
}) {
  const t = useTranslations("companion");
  const value = setting.refreshRate;
  const [draft, setDraft] = useState(value);
  const [localTick, setLocalTick] = useState(0);
  const onUpdateRef = useRef(onUpdate);
  const valueKey = useRef(refreshKey(value));
  const controlled = useRef(savedTick !== undefined);
  const tick = savedTick ?? localTick;
  const disabled = pending;

  useEffect(() => {
    onUpdateRef.current = onUpdate;
  }, [onUpdate]);

  useEffect(() => {
    valueKey.current = refreshKey(value);
    controlled.current = savedTick !== undefined;
  }, [savedTick, value]);

  useEffect(() => {
    if (disabled || refreshKey(draft) === valueKey.current) return;
    const timer = window.setTimeout(() => {
      if (refreshKey(draft) === valueKey.current) return;
      onUpdateRef.current(draft);
      if (!controlled.current) setLocalTick((next) => next + 1);
    }, SAVE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [draft, disabled]);

  function addFileType(extension: string) {
    setDraft((current) => ({
      ...current,
      byKind: [
        ...current.byKind,
        {
          extension,
          regular: current.regular,
          inverted: current.inverted,
        },
      ],
      available: current.available.filter((item) => item !== extension),
    }));
  }

  function removeFileType(extension: string) {
    setDraft((current) => ({
      ...current,
      byKind: current.byKind.filter((item) => item.extension !== extension),
      available: [...current.available, extension].sort(),
    }));
  }

  return (
    <div className="flex flex-col gap-4 rounded-xl border border-kumo-line bg-kumo-base p-4">
      <div className="flex items-center justify-between gap-3">
        <Text aria-hidden as="span">
          {setting.label}
        </Text>
        <span aria-live="polite" className="inline-flex w-5 justify-center">
          <SavedMark label={t("saved")} tick={tick} />
        </span>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <RateField
          disabled={disabled}
          label={t("regular")}
          value={draft.regular}
          onChange={(regular) =>
            setDraft((current) => ({ ...current, regular }))
          }
        />
        <RateField
          disabled={disabled}
          label={t("inverted")}
          value={draft.inverted}
          onChange={(inverted) =>
            setDraft((current) => ({ ...current, inverted }))
          }
        />
      </div>
      <Text variant="secondary" size="sm">
        {t("refreshHint")}
      </Text>
      {draft.byKind.map((entry) => (
        <div
          key={entry.extension}
          className="grid items-end gap-3 border-t border-kumo-line pt-3 sm:grid-cols-[6rem_1fr_1fr_auto]"
        >
          <Text>{extensionLabel(entry.extension)}</Text>
          <RateField
            ariaLabel={`${extensionLabel(entry.extension)} ${t("regular")}`}
            disabled={disabled}
            label={t("regular")}
            value={entry.regular}
            onChange={(regular) =>
              setDraft((current) => ({
                ...current,
                byKind: current.byKind.map((item) =>
                  item.extension === entry.extension
                    ? { ...item, regular }
                    : item,
                ),
              }))
            }
          />
          <RateField
            ariaLabel={`${extensionLabel(entry.extension)} ${t("inverted")}`}
            disabled={disabled}
            label={t("inverted")}
            value={entry.inverted}
            onChange={(inverted) =>
              setDraft((current) => ({
                ...current,
                byKind: current.byKind.map((item) =>
                  item.extension === entry.extension
                    ? { ...item, inverted }
                    : item,
                ),
              }))
            }
          />
          <Button
            disabled={disabled}
            size="sm"
            type="button"
            variant="secondary"
            onClick={() => removeFileType(entry.extension)}
          >
            {t("remove")}
          </Button>
        </div>
      ))}
      {draft.available.length > 0 ? (
        <Select
          key={draft.available.join("\n")}
          aria-label={t("addFileType")}
          disabled={disabled}
          placeholder={t("addFileType")}
          onValueChange={(next) => {
            if (typeof next === "string" && next) addFileType(next);
          }}
        >
          {[...draft.available].sort().map((extension) => (
            <Select.Option key={extension} value={extension}>
              {extensionLabel(extension)}
            </Select.Option>
          ))}
        </Select>
      ) : null}
    </div>
  );
}

function RateField({
  label,
  value,
  disabled,
  onChange,
  ariaLabel = label,
}: {
  label: string;
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
  ariaLabel?: string;
}) {
  return (
    <label className="flex min-w-0 flex-col gap-1">
      <Text variant="secondary" size="sm">
        {label}
      </Text>
      <Input
        aria-label={ariaLabel}
        disabled={disabled}
        inputMode="numeric"
        passwordManagerIgnore
        value={String(value)}
        onBeforeInput={(event) =>
          blockDisallowedBeforeInput(event, rateAllowed)
        }
        onChange={(event) => {
          const next = event.target.value;
          if (next === "") {
            onChange(0);
            return;
          }
          if (!rateAllowed(next)) {
            event.currentTarget.value = String(value);
            return;
          }
          onChange(Number(next));
        }}
        onKeyDown={(event) => blockDisallowedKey(event, rateAllowed)}
        onPaste={(event) => blockDisallowedPaste(event, rateAllowed)}
      />
    </label>
  );
}

function flipCheckItem(items: CheckItem[], id: string): CheckItem[] {
  return items.map((item) =>
    item.id === id ? { ...item, on: !item.on } : item,
  );
}

function choiceLabel(choice: SettingChoice, value: unknown) {
  if (typeof value !== "string" || value === "") return null;
  return choice.options.find((option) => option.id === value)?.label ?? value;
}

function fillHex(choice: SettingChoice): string {
  return /^#[0-9a-fA-F]{6}$/.test(choice.selected)
    ? choice.selected.toLowerCase()
    : "#ffffff";
}

function FillColorControl({
  label,
  choice,
  disabled,
  placeholder,
  rgbLabel,
  onUpdate,
}: {
  label: string;
  choice: SettingChoice;
  disabled: boolean;
  placeholder: string;
  rgbLabel: string;
  onUpdate: (value: string) => void;
}) {
  const trimmed = fillColorChoice(choice);
  return (
    <div className="flex w-full items-center gap-2 sm:w-auto">
      <ChoiceSelect
        choice={trimmed}
        disabled={disabled}
        label={label}
        placeholder={placeholder}
        onUpdate={onUpdate}
      />
      <input
        aria-label={rgbLabel}
        className="h-10 w-12 shrink-0 cursor-pointer rounded-lg border border-kumo-line bg-kumo-base p-1"
        disabled={disabled}
        type="color"
        value={fillHex(trimmed)}
        onChange={(event) => onUpdate(event.target.value)}
      />
    </div>
  );
}

function isDevicePath(value: string) {
  return value.startsWith("/") || value.includes("/");
}

function parentDir(path: string) {
  const index = path.lastIndexOf("/");
  if (index <= 0) return path.startsWith("/") ? "/" : "";
  return path.slice(0, index);
}

function ImageChoiceControl({
  label,
  choice,
  disabled,
  onGetFile,
  onListDirectory,
  onUpdate,
}: {
  label: string;
  choice: SettingChoice;
  disabled: boolean;
  onGetFile?: (path: string) => Promise<Blob>;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (value: string) => void;
}) {
  const t = useTranslations("companion");
  const [open, setOpen] = useState(false);
  const custom = isDevicePath(choice.selected);

  return (
    <div className="flex w-full flex-col items-stretch gap-2 sm:w-auto">
      <div className="flex w-full items-center gap-2">
        <ChoiceSelect
          choice={choice}
          disabled={disabled}
          label={label}
          placeholder={t("choose")}
          onUpdate={onUpdate}
        />
        <Button
          disabled={disabled || !onListDirectory}
          size="sm"
          type="button"
          variant="secondary"
          onClick={() => setOpen(true)}
        >
          {t("browseImage")}
        </Button>
      </div>
      {custom ? (
        <ImagePreview
          key={choice.selected}
          path={choice.selected}
          onGetFile={onGetFile}
        />
      ) : null}
      {open && onListDirectory ? (
        <DirectoryBrowser
          extensions={IMAGE_EXTENSIONS}
          initialPath={custom ? parentDir(choice.selected) : ""}
          listDirectory={onListDirectory}
          mode="file"
          onClose={() => setOpen(false)}
          onGetFile={onGetFile}
          onSelect={(path) => {
            setOpen(false);
            onUpdate(path);
          }}
        />
      ) : null}
    </div>
  );
}

function useFilePreview(
  path: string,
  onGetFile?: (path: string) => Promise<Blob>,
) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!onGetFile) return;
    let cancelled = false;
    let objectUrl: string | undefined;
    onGetFile(path)
      .then((blob) => {
        const url = URL.createObjectURL(blob);
        if (cancelled) {
          URL.revokeObjectURL(url);
          return;
        }
        objectUrl = url;
        setSrc(url);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [onGetFile, path]);

  return {
    src: onGetFile ? src : null,
    failed: !onGetFile || failed,
  };
}

function ImagePreview({
  path,
  onGetFile,
}: {
  path: string;
  onGetFile?: (path: string) => Promise<Blob>;
}) {
  const t = useTranslations("companion");
  const { src, failed } = useFilePreview(path, onGetFile);

  if (!onGetFile || failed) {
    return <Text variant="error">{t("previewError")}</Text>;
  }
  if (!src) {
    return (
      <div className="flex justify-center py-4">
        <Loader />
      </div>
    );
  }
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      alt={t("preview")}
      className="max-h-40 max-w-full rounded-lg border border-kumo-line object-contain"
      src={src}
    />
  );
}

function FileThumb({
  path,
  onGetFile,
}: {
  path: string;
  onGetFile?: (path: string) => Promise<Blob>;
}) {
  const { src, failed } = useFilePreview(path, onGetFile);
  if (!src || failed) {
    return (
      <span className="flex h-24 items-center justify-center">
        <ImageIcon aria-hidden className="text-kumo-subtle" size={40} />
      </span>
    );
  }
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img alt="" className="h-24 w-full rounded-md object-contain" src={src} />
  );
}

function ChoiceSelect({
  label,
  choice,
  disabled,
  placeholder,
  onUpdate,
}: {
  label: string;
  choice: SettingChoice;
  disabled: boolean;
  placeholder: string;
  onUpdate: (value: string) => void;
}) {
  return (
    <Select
      aria-label={label}
      className="w-full sm:w-64"
      disabled={disabled}
      placeholder={placeholder}
      renderValue={(value) => choiceLabel(choice, value)}
      value={choice.selected}
      onValueChange={(value) => {
        if (typeof value === "string") onUpdate(value);
      }}
    >
      {choice.options.map((option) => (
        <Select.Option key={option.id} value={option.id}>
          {option.label}
        </Select.Option>
      ))}
    </Select>
  );
}

function ChecksSetting({
  setting,
  pending,
  onUpdate,
}: {
  setting: Extract<Setting, { kind: "checks" }>;
  pending: boolean;
  onUpdate: (value: SettingInput) => void;
}) {
  const disabled = pending;

  return (
    <div className="flex flex-col gap-3 rounded-xl border border-kumo-line bg-kumo-base p-4">
      <div className="flex min-w-0 flex-col gap-1">
        <Text aria-hidden as="span">
          {setting.label}
        </Text>
      </div>
      {setting.items.map((item) => (
        <div key={item.id} className="flex items-center justify-between gap-3">
          <Text>{item.label}</Text>
          <Switch
            aria-label={item.label}
            checked={item.on}
            disabled={disabled}
            onCheckedChange={() =>
              onUpdate({ items: flipCheckItem(setting.items, item.id) })
            }
          />
        </div>
      ))}
    </div>
  );
}

type PathCrumb = { name: string; path: string };

function pathCrumbs(path: string): PathCrumb[] {
  if (!path || path === ".") return [];
  const rooted = path.startsWith("/");
  const parts = path.split("/").filter(Boolean);
  const crumbs: PathCrumb[] = rooted ? [{ name: "/", path: "/" }] : [];
  let acc = "";
  for (const part of parts) {
    acc = rooted ? `${acc}/${part}` : acc ? `${acc}/${part}` : part;
    crumbs.push({ name: part, path: acc });
  }
  return crumbs;
}

function visibleCrumbs(crumbs: PathCrumb[]): (PathCrumb | "gap")[] {
  if (crumbs.length <= 4) return crumbs;
  const first = crumbs[0];
  const prev = crumbs[crumbs.length - 2];
  const last = crumbs[crumbs.length - 1];
  if (!first || !prev || !last) return crumbs;
  return [first, "gap", prev, last];
}

function CrumbNav({
  crumbs,
  onOpen,
}: {
  crumbs: (PathCrumb | "gap")[];
  onOpen: (path: string) => void;
}) {
  if (crumbs.length === 0) return null;
  const last = crumbs.length - 1;
  return (
    <nav
      aria-label={crumbs
        .filter((crumb): crumb is PathCrumb => crumb !== "gap")
        .map((crumb) => crumb.name)
        .join(" / ")}
      className="flex min-w-0 flex-wrap items-center gap-0.5 rounded-lg bg-kumo-tint px-1"
    >
      {crumbs.map((crumb, index) => {
        if (crumb === "gap") {
          return (
            <Text key={`gap-${index}`} variant="secondary">
              …
            </Text>
          );
        }
        if (index === last) {
          return <Text key={crumb.path}>{crumb.name}</Text>;
        }
        return (
          <Button
            key={crumb.path}
            size="sm"
            type="button"
            variant="ghost"
            onClick={() => onOpen(crumb.path)}
          >
            {crumb.name}
          </Button>
        );
      })}
    </nav>
  );
}

function FileBrowserGrid({
  entries,
  onGetFile,
  onOpenDir,
  onSelect,
}: {
  entries: DirectoryEntry[];
  onGetFile?: (path: string) => Promise<Blob>;
  onOpenDir: (path: string) => void;
  onSelect: (path: string) => void;
}) {
  const directories = entries.filter((entry) => entry.directory);
  const files = entries.filter((entry) => !entry.directory);

  return (
    <ul className="grid grid-cols-2 gap-2 p-2 sm:grid-cols-3">
      {directories.map((entry) => (
        <li key={entry.path}>
          <Button
            className="h-full w-full flex-col items-center justify-center gap-2 py-4"
            icon={FolderSimpleIcon}
            type="button"
            variant="ghost"
            onClick={() => onOpenDir(entry.path)}
          >
            <span className="w-full truncate text-center">{entry.name}</span>
          </Button>
        </li>
      ))}
      {files.map((entry) => (
        <li key={entry.path}>
          <Button
            className="h-full w-full flex-col items-stretch gap-2 py-2"
            type="button"
            variant="ghost"
            onClick={() => onSelect(entry.path)}
          >
            <FileThumb path={entry.path} onGetFile={onGetFile} />
            <span className="w-full truncate text-center text-sm">
              {entry.name}
            </span>
          </Button>
        </li>
      ))}
    </ul>
  );
}

export function DirectoryBrowser({
  listDirectory,
  initialPath = "",
  mode = "directory",
  extensions,
  onGetFile,
  onSelect,
  onClose,
}: {
  listDirectory: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  initialPath?: string;
  mode?: "directory" | "file";
  extensions?: string[];
  onGetFile?: (path: string) => Promise<Blob>;
  onSelect: (path: string) => void;
  onClose: () => void;
}) {
  const t = useTranslations("companion");
  const [path, setPath] = useState(initialPath);
  const [result, setResult] = useState<{
    request: string;
    listing: DirectoryListing;
  } | null>(null);
  const [failedPath, setFailedPath] = useState<string | null>(null);
  const fileMode = mode === "file";
  const listedExtensions =
    fileMode && extensions && extensions.length > 0 ? extensions : undefined;

  useEffect(() => {
    let cancelled = false;
    listDirectory(path, listedExtensions)
      .then((listing) => {
        if (cancelled) return;
        setResult({ request: path, listing });
        setFailedPath(null);
      })
      .catch(() => {
        if (!cancelled) setFailedPath(path);
      });
    return () => {
      cancelled = true;
    };
  }, [listDirectory, listedExtensions, path]);

  const listing = result?.request === path ? result.listing : null;
  const current = path || listing?.path || "";
  const error = failedPath === path;
  const loading = !error && result?.request !== path;
  const entries = listing?.entries ?? [];
  const shown = fileMode ? entries : entries.filter((entry) => entry.directory);

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <Dialog className="flex flex-col gap-4 p-6" size="lg">
        <Dialog.Title className="text-lg font-semibold">
          {fileMode ? t("selectImage") : t("browseFolder")}
        </Dialog.Title>
        <Dialog.Description className="sr-only">
          {current || (fileMode ? t("browseImage") : t("browse"))}
        </Dialog.Description>
        <CrumbNav
          crumbs={visibleCrumbs(pathCrumbs(current))}
          onOpen={setPath}
        />
        {error ? <Text variant="error">{t("browseError")}</Text> : null}
        <div
          aria-busy={loading}
          className={
            fileMode
              ? "max-h-[28rem] min-h-48 overflow-y-auto rounded-lg bg-kumo-tint [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
              : "max-h-72 min-h-40 overflow-y-auto rounded-lg bg-kumo-tint [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
          }
        >
          {loading ? (
            <div className="flex justify-center py-8">
              <Loader />
            </div>
          ) : null}
          {!loading && !error && shown.length === 0 ? (
            <Empty
              icon={fileMode ? <ImageIcon /> : <FolderSimpleIcon />}
              size="sm"
              title={fileMode ? t("emptyImages") : t("emptyFolder")}
            />
          ) : null}
          {!loading && shown.length > 0 ? (
            fileMode ? (
              <FileBrowserGrid
                entries={shown}
                onGetFile={onGetFile}
                onOpenDir={setPath}
                onSelect={onSelect}
              />
            ) : (
              <ul className="flex flex-col p-1">
                {shown.map((entry) => (
                  <li key={entry.path}>
                    <Button
                      className="w-full justify-start"
                      icon={FolderSimpleIcon}
                      type="button"
                      variant="ghost"
                      onClick={() => setPath(entry.path)}
                    >
                      <span className="flex min-w-0 flex-1 items-center justify-between gap-2">
                        <span className="truncate">{entry.name}</span>
                        <CaretRightIcon
                          aria-hidden
                          className="text-kumo-subtle"
                          size={16}
                        />
                      </span>
                    </Button>
                  </li>
                ))}
              </ul>
            )
          ) : null}
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button type="button" variant="secondary" onClick={onClose}>
            {t("cancel")}
          </Button>
          {fileMode ? null : (
            <Button
              disabled={!current || loading || error}
              type="button"
              onClick={() => onSelect(current)}
            >
              {t("selectFolder")}
            </Button>
          )}
        </div>
      </Dialog>
    </Dialog.Root>
  );
}

function PathBrowser({
  disabled,
  path,
  onListDirectory,
  onSelect,
}: {
  disabled: boolean;
  path: string;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onSelect: (path: string) => void;
}) {
  const t = useTranslations("companion");
  const [open, setOpen] = useState(false);

  return (
    <div className="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-center">
      <Text variant="secondary" size="sm">
        {path || t("browse")}
      </Text>
      <Button
        disabled={disabled || !onListDirectory}
        size="sm"
        type="button"
        variant="secondary"
        onClick={() => setOpen(true)}
      >
        {t("browse")}
      </Button>
      {open && onListDirectory ? (
        <DirectoryBrowser
          initialPath={path}
          listDirectory={onListDirectory}
          onClose={() => setOpen(false)}
          onSelect={(next) => {
            setOpen(false);
            onSelect(next);
          }}
        />
      ) : null}
    </div>
  );
}

function PathSetting({
  setting,
  pending,
  onListDirectory,
  onUpdate,
}: {
  setting: Extract<Setting, { kind: "path" }>;
  pending: boolean;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (value: SettingInput) => void;
}) {
  const disabled = pending;

  return (
    <div className="flex flex-col gap-3 rounded-xl border border-kumo-line bg-kumo-base p-4">
      <div className="flex min-w-0 flex-col gap-1">
        <Text aria-hidden as="span">
          {setting.label}
        </Text>
      </div>
      <PathBrowser
        disabled={disabled}
        path={setting.path}
        onListDirectory={onListDirectory}
        onSelect={(path) => onUpdate({ path })}
      />
    </div>
  );
}

function LibrarySetting({
  setting,
  pending,
  savedTick,
  onListDirectory,
  onUpdate,
}: {
  setting: Extract<Setting, { kind: "library" }>;
  pending: boolean;
  savedTick?: number;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (value: SettingInput) => void;
}) {
  const t = useTranslations("companion");
  const library = setting.library;
  const disabled = pending;

  function updateLibrary(next: LibraryValue) {
    onUpdate(next);
  }

  return (
    <div className="flex flex-col gap-4 rounded-xl border border-kumo-line bg-kumo-base p-4">
      <div className="flex min-w-0 flex-col gap-1">
        <Text aria-hidden as="span">
          {setting.label}
        </Text>
      </div>
      <TextSetting
        label={setting.label}
        mode="text"
        disabled={disabled}
        savedLabel={t("saved")}
        savedTick={savedTick}
        value={library.name}
        onUpdate={(name) => updateLibrary({ ...library, name })}
      />
      <PathBrowser
        disabled={disabled}
        path={library.path.path}
        onListDirectory={onListDirectory}
        onSelect={(path) => updateLibrary({ ...library, path: { path } })}
      />
      <ChoiceSelect
        choice={library.finished}
        disabled={disabled}
        label={setting.label}
        placeholder={t("choose")}
        onUpdate={(selected) =>
          updateLibrary({
            ...library,
            finished: { ...library.finished, selected },
          })
        }
      />
      <Button
        disabled={disabled}
        size="sm"
        type="button"
        variant="secondary"
        onClick={() => updateLibrary({ ...library, remove: true })}
      >
        {t("remove")}
      </Button>
    </div>
  );
}

function SavedMark({ tick, label }: { tick: number; label: string }) {
  if (tick <= 0) return null;
  return (
    <>
      <CheckIcon
        key={tick}
        aria-hidden
        className="companion-saved text-kumo-link"
        size={18}
        weight="bold"
      />
      <span className="sr-only">{label}</span>
    </>
  );
}
