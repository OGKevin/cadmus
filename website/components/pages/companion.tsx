"use client";

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Button, LinkButton } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Text } from "@cloudflare/kumo/components/text";
import { useLocale, useTranslations } from "next-intl";
import { CategoryList } from "@/components/companion-categories/index";
import { CompanionInstructions } from "@/components/companion-instructions/index";
import { LanguageSwitcher } from "@/components/language-switcher/index";
import { apiHref, guideHref, rustApiHref } from "@/lib/doc-hrefs";
import { DocsMenu } from "@/components/docs-menu/index";
import {
  addLibrary,
  applySetting,
  getFile,
  isLibraryRemove,
  listDirectory,
  isUnauthorized,
  listSettings,
  probeCompanion,
  removeLibrary,
  updateSetting,
  type Category,
  type CompanionProbe,
  type CompanionStatus,
  type SettingInput,
} from "@/lib/companion";

const TOKEN_KEY = "cadmus-companion-token";

function errorDetail(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function CompanionPage({
  token,
  categories,
  status,
  view: initialView = "home",
  onToken,
  onTokenRejected,
  tokenRejected,
}: {
  token?: string;
  categories?: Category[];
  status?: CompanionStatus;
  view?: "home" | "settings";
  onToken?: (token: string) => void;
  onTokenRejected?: () => void;
  tokenRejected?: boolean;
}) {
  const probe = useCompanionProbe(status === undefined);
  const shown = status ? { state: "up" as const, ...status } : probe;
  const [view, setView] = useState(initialView);
  const version = shown?.state === "up" ? shown.version : "";

  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-kumo-base text-kumo-default">
      <CompanionTopBar
        settingsCurrent={view === "settings"}
        showSettings={Boolean(token || categories)}
        version={version}
        onHome={() => setView("home")}
        onSettings={() => setView("settings")}
      />
      <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden px-6 py-8">
        <CompanionBody
          categories={categories}
          probe={shown}
          token={token}
          view={view}
          onSettings={() => setView("settings")}
          onToken={onToken}
          onTokenRejected={onTokenRejected}
          tokenRejected={tokenRejected}
        />
      </main>
    </div>
  );
}

function useCompanionProbe(enabled: boolean): CompanionProbe | null {
  const [probe, setProbe] = useState<CompanionProbe | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let timer = 0;

    function schedule() {
      timer = window.setTimeout(() => {
        void tick();
      }, 5000);
    }

    async function tick() {
      try {
        const next = await probeCompanion();
        if (cancelled) return;
        setProbe(next);
        if (next.state === "down") schedule();
      } catch {
        if (!cancelled) setProbe({ state: "absent" });
      }
    }

    void tick();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [enabled]);

  return probe;
}

function CompanionTopBar({
  settingsCurrent,
  showSettings,
  version,
  onHome,
  onSettings,
}: {
  settingsCurrent: boolean;
  showSettings: boolean;
  version: string;
  onHome: () => void;
  onSettings: () => void;
}) {
  const t = useTranslations("companion");

  return (
    <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center border-b border-kumo-line bg-kumo-base px-4">
      <button
        type="button"
        className="absolute left-1/2 -translate-x-1/2 text-sm font-medium"
        onClick={onHome}
      >
        {version ? `Cadmus ${version}` : t("home")}
      </button>
      <DocsMenu
        settingsCurrent={settingsCurrent}
        showSettings={showSettings}
        onSettings={onSettings}
      />
      <div className="ml-auto">
        <LanguageSwitcher />
      </div>
    </header>
  );
}

function CompanionBody({
  token,
  categories,
  probe,
  view,
  onSettings,
  onToken,
  onTokenRejected,
  tokenRejected,
}: {
  token?: string;
  categories?: Category[];
  probe: CompanionProbe | null;
  view: "home" | "settings";
  onSettings: () => void;
  onToken?: (token: string) => void;
  onTokenRejected?: () => void;
  tokenRejected?: boolean;
}) {
  const t = useTranslations("companion");
  let body;
  if (view === "settings" && categories) {
    body = <CompanionEditor categories={categories} />;
  } else if (!probe) {
    body = <Text variant="secondary">{t("loading")}</Text>;
  } else if (probe.state === "down") {
    body = <CompanionInstructions message="down" />;
  } else if (probe.state !== "up") {
    body = <CompanionInstructions />;
  } else if (!token) {
    body = onToken ? (
      <TokenPrompt invalid={tokenRejected} onToken={onToken} />
    ) : (
      <CompanionInstructions />
    );
  } else if (view === "settings") {
    body = <CompanionEditor token={token} onTokenRejected={onTokenRejected} />;
  } else {
    body = <CompanionHome status={probe} onSettings={onSettings} />;
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">{body}</div>
  );
}

function CompanionHome({
  status,
  onSettings,
}: {
  status: CompanionStatus;
  onSettings: () => void;
}) {
  const t = useTranslations("companion");
  const docs = useTranslations("docs");
  const locale = useLocale();

  return (
    <div className="mx-auto flex min-h-0 w-full max-w-3xl flex-1 flex-col gap-8 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      <div className="grid gap-4 sm:grid-cols-3">
        <Info label={t("deviceName")} value={status.hostname} />
        <Info label={t("deviceLanguage")} value={status.locale} />
        <Info label={t("version")} value={status.version} />
      </div>
      <div className="flex flex-wrap gap-3">
        <LinkButton href={guideHref(locale)} variant="secondary">
          {docs("userGuide.label")}
        </LinkButton>
        <LinkButton href={apiHref(locale)} variant="secondary">
          {docs("apiReference.label")}
        </LinkButton>
        <LinkButton href={rustApiHref(locale)} variant="secondary">
          {docs("rustApi.label")}
        </LinkButton>
        <Button type="button" onClick={onSettings}>
          {t("settings")}
        </Button>
      </div>
    </div>
  );
}

function Info({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-xl border border-kumo-line p-4">
      <Text variant="secondary" size="sm">
        {label}
      </Text>
      <div className="break-all">
        <Text>{value}</Text>
      </div>
    </div>
  );
}

const tokenListeners = new Set<() => void>();

function subscribeToken(listener: () => void) {
  tokenListeners.add(listener);
  return () => {
    tokenListeners.delete(listener);
  };
}

function readStoredToken() {
  try {
    return window.localStorage.getItem(TOKEN_KEY)?.trim() ?? "";
  } catch {
    return "";
  }
}

function notifyToken() {
  for (const listener of tokenListeners) listener();
}

export function CompanionQuery() {
  const token = useSyncExternalStore(subscribeToken, readStoredToken, () => "");
  const [rejected, setRejected] = useState(false);

  function remember(next: string) {
    try {
      window.localStorage.setItem(TOKEN_KEY, next);
    } catch {
      /* ignore quota / private mode */
    }
    setRejected(false);
    notifyToken();
  }

  function forget() {
    try {
      window.localStorage.removeItem(TOKEN_KEY);
    } catch {
      /* ignore quota / private mode */
    }
    setRejected(true);
    notifyToken();
  }

  return (
    <CompanionPage
      token={token || undefined}
      tokenRejected={rejected}
      onToken={remember}
      onTokenRejected={forget}
    />
  );
}

function TokenPrompt({
  invalid,
  onToken,
}: {
  invalid?: boolean;
  onToken: (token: string) => void;
}) {
  const t = useTranslations("companion");
  const [draft, setDraft] = useState("");

  return (
    <form
      className="mx-auto flex w-full max-w-sm flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        const next = draft.trim();
        if (/^\d{6}$/.test(next)) onToken(next);
      }}
    >
      <Text>{invalid ? t("tokenInvalid") : t("tokenPrompt")}</Text>
      <Input
        aria-label={t("tokenPrompt")}
        autoComplete="one-time-code"
        inputMode="numeric"
        maxLength={6}
        passwordManagerIgnore
        value={draft}
        onChange={(event) => {
          const next = event.target.value.replace(/\D/g, "").slice(0, 6);
          setDraft(next);
        }}
      />
      <Button disabled={!/^\d{6}$/.test(draft)} type="submit">
        {t("tokenSubmit")}
      </Button>
    </form>
  );
}

function CompanionEditor({
  token,
  categories: initialCategories,
  onTokenRejected,
}: {
  token?: string;
  categories?: Category[];
  onTokenRejected?: () => void;
}) {
  const t = useTranslations("companion");
  const live = initialCategories === undefined;
  const [categories, setCategories] = useState<Category[] | null>(
    initialCategories ?? null,
  );
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const saving = useRef(false);
  const queued = useRef(new Map<string, SettingInput>());
  const alive = useRef(true);
  const categoriesRef = useRef(initialCategories ?? null);
  const [saved, setSaved] = useState<{ id: string; tick: number } | null>(null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!live || !token) return;
    let cancelled = false;
    listSettings(token)
      .then((next) => {
        if (cancelled) return;
        categoriesRef.current = next;
        setCategories(next);
      })
      .catch((error) => {
        if (cancelled) return;
        if (isUnauthorized(error)) {
          onTokenRejected?.();
          return;
        }
        setLoadError(errorDetail(error));
      });
    return () => {
      cancelled = true;
    };
  }, [live, token, attempt, onTokenRejected]);

  function retry() {
    categoriesRef.current = null;
    setCategories(null);
    setLoadError(null);
    setAttempt((value) => value + 1);
  }

  function noteSaved(id: string) {
    setSaved((current) => ({
      id,
      tick: current?.id === id ? current.tick + 1 : 1,
    }));
  }

  function finishSave(id: string, failed: boolean) {
    if (!alive.current) return;
    saving.current = false;
    setPendingId(null);
    if (!failed) noteSaved(id);
    const follow = queued.current.entries().next().value;
    if (follow) {
      queued.current.delete(follow[0]);
      onUpdate(follow[0], follow[1]);
    }
  }

  async function reloadSettings() {
    if (!token) return;
    const next = await listSettings(token);
    if (!alive.current) return;
    categoriesRef.current = next;
    setCategories(next);
  }

  const onListDirectory = useCallback(
    (path: string, extensions?: string[]) => {
      if (!token) return Promise.reject(new Error("Companion request failed"));
      return listDirectory(token, path, extensions);
    },
    [token],
  );

  const onGetFile = useCallback(
    (path: string) => {
      if (!token) return Promise.reject(new Error("Companion request failed"));
      return getFile(token, path);
    },
    [token],
  );

  function onAddLibrary(path: string) {
    if (saving.current || !token) return;
    saving.current = true;
    setPendingId("AddLibrary");
    setSaveError(null);
    addLibrary(token, {
      name: "",
      path: { path },
      finished: { selected: "", options: [] },
    })
      .then(() => reloadSettings())
      .then(() => finishSave("AddLibrary", false))
      .catch((error) => {
        if (alive.current) setSaveError(errorDetail(error));
        finishSave("AddLibrary", true);
      });
  }

  function onUpdate(id: string, value: SettingInput) {
    if (saving.current) {
      queued.current.set(id, value);
      return;
    }
    const current = categoriesRef.current;
    if (!current) return;
    const setting = current
      .flatMap((category) => category.settings)
      .find((item) => item.id === id);
    if (!setting) return;

    const previous = current;
    const next = applySetting(current, id, value);
    categoriesRef.current = next;
    saving.current = true;
    setPendingId(id);
    setSaveError(null);
    setCategories(next);

    if (!token) {
      finishSave(id, false);
      return;
    }

    const request = isLibraryRemove(value)
      ? removeLibrary(token, setting)
      : updateSetting(token, setting, value);
    request
      .then(() => finishSave(id, false))
      .catch((error) => {
        if (!alive.current) return;
        if (isUnauthorized(error)) {
          onTokenRejected?.();
          return;
        }
        categoriesRef.current = previous;
        setCategories(previous);
        setSaveError(errorDetail(error));
        finishSave(id, true);
      });
  }

  return (
    <div className="mx-auto flex min-h-0 w-full max-w-3xl flex-1 flex-col gap-6 overflow-hidden">
      {loadError ? (
        <div className="flex flex-col items-start gap-3">
          <Text variant="error">{t("loadError")}</Text>
          <Text variant="secondary">{loadError}</Text>
          <Button type="button" variant="secondary" onClick={retry}>
            {t("retry")}
          </Button>
        </div>
      ) : null}
      {!loadError && !categories ? (
        <Text variant="secondary">{t("loading")}</Text>
      ) : null}
      {categories ? (
        <>
          {saveError ? (
            <>
              <Text variant="error">{t("saveError")}</Text>
              <Text variant="secondary">{saveError}</Text>
            </>
          ) : null}
          <CategoryList
            categories={categories}
            pendingId={pendingId}
            saved={saved}
            onAddLibrary={token ? onAddLibrary : undefined}
            onGetFile={token ? onGetFile : undefined}
            onListDirectory={token ? onListDirectory : undefined}
            onUpdate={onUpdate}
          />
        </>
      ) : null}
    </div>
  );
}
