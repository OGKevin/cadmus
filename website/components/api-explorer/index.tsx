"use client";

import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { SidebarSimple } from "@phosphor-icons/react";
import type { Folder, Item, Root } from "fumadocs-core/page-tree";
import type { AuthField } from "fumadocs-openapi/playground";
import { createOpenAPIPage } from "fumadocs-openapi/ui";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import {
  SidebarTrigger,
  useSidebar,
} from "fumadocs-ui/layouts/docs/slots/sidebar";
import { useTranslations } from "next-intl";
import { DocsMenu } from "@/components/docs-menu/index";
import { LanguageSwitcher } from "@/components/language-switcher/index";
import {
  PREFILL_KEY,
  shouldReplaceSessionToken,
  TOKEN_KEY,
} from "@/lib/companion-token";
import {
  groupOperations,
  useFragment,
  type ApiGroup,
  type HttpMethod,
  type OpenAPISpec,
} from "./navigation";

const SPEC_URL = `${process.env.NEXT_PUBLIC_BASE_PATH ?? ""}${
  process.env.NEXT_PUBLIC_OPENAPI_URL ?? "/openapi.json"
}`;

/** Gives our header its row on every width; Fumadocs only sizes it on mobile. */
const HEADER_HEIGHT = { "--fd-header-height": "3.5rem" } as CSSProperties;

/**
 * Fills the bearer field with the companion token saved in localStorage.
 *
 * The playground keeps what the field held in `sessionStorage` for the tab and
 * prefers it over `defaultValue`, so an empty value left from before pairing
 * would hide the token. A blank tab value is replaced with the token; a token
 * the user typed in this tab is kept.
 */
function prefillCompanionToken(fields: AuthField[]): AuthField[] {
  const token = window.localStorage.getItem(TOKEN_KEY)?.trim() ?? "";
  if (!token) return fields;
  const lastPrefill = window.sessionStorage.getItem(PREFILL_KEY);

  return fields.map((field) => {
    if (field.schemeId !== "bearerAuth") return field;
    const stored = window.sessionStorage.getItem(field.storageKey);
    const current: unknown = stored ? JSON.parse(stored) : "";
    if (shouldReplaceSessionToken(current, lastPrefill)) {
      window.sessionStorage.setItem(field.storageKey, JSON.stringify(token));
      window.sessionStorage.setItem(PREFILL_KEY, token);
    }
    return { ...field, defaultValue: token };
  });
}

const OpenAPIPage = createOpenAPIPage({
  playground: {
    transformAuthInputs: prefillCompanionToken,
  },
});

function MethodLabel({ method }: { method: HttpMethod }) {
  return (
    <span className="w-14 shrink-0 font-mono text-[11px] font-semibold uppercase text-fd-muted-foreground">
      {method}
    </span>
  );
}

/**
 * One sidebar folder per tag, one entry per operation.
 *
 * `DocsLayout` keeps a tree until its `$id` changes, so the id lists the
 * operations: the empty tree shown while the spec loads is then replaced.
 */
function buildPageTree(groups: ApiGroup[]): Root {
  return {
    $id: groups
      .flatMap((group) => group.operations.map((operation) => operation.id))
      .join(" "),
    name: "Cadmus API",
    children: groups.map((group): Folder => ({
      type: "folder",
      name: group.label,
      defaultOpen: true,
      children: group.operations.map((operation): Item => ({
        type: "page",
        name: operation.title,
        url: `#${operation.id}`,
        icon: <MethodLabel method={operation.method} />,
      })),
    })),
  };
}

function SidebarOperation({ item }: { item: Item }) {
  const fragment = useFragment();
  const { setOpen } = useSidebar();
  const current = item.url === `#${fragment}`;

  return (
    <a
      href={item.url}
      aria-current={current ? "page" : undefined}
      onClick={() => setOpen(false)}
      className={`flex items-center gap-2 rounded-lg p-2 text-sm ${
        current
          ? "bg-fd-accent font-medium text-fd-accent-foreground"
          : "text-fd-muted-foreground hover:bg-fd-accent/50 hover:text-fd-accent-foreground"
      }`}
    >
      {item.icon}
      <span className="min-w-0 truncate">{item.name}</span>
    </a>
  );
}

function ApiHeader() {
  const t = useTranslations("apiExplorer");

  return (
    <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b border-fd-border bg-fd-background px-4 [grid-area:header]">
      <SidebarTrigger
        aria-label={t("operations")}
        className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg border border-fd-border text-fd-foreground md:hidden"
      >
        <SidebarSimple size={20} weight="duotone" aria-hidden />
      </SidebarTrigger>
      <DocsMenu />
      <a
        href="#"
        className="absolute left-1/2 max-w-[40%] -translate-x-1/2 truncate text-sm font-medium"
      >
        {t("title")}
      </a>
      <div className="ml-auto">
        <LanguageSwitcher />
      </div>
    </header>
  );
}

function TagDescription({ group }: { group: ApiGroup }) {
  return (
    <div className="flex flex-col gap-2">
      <h2 className="text-lg font-semibold">{group.label}</h2>
      <p className="font-mono text-xs text-fd-muted-foreground">{group.name}</p>
      {group.description ? (
        <p className="whitespace-pre-line text-sm text-fd-muted-foreground">
          {group.description}
        </p>
      ) : null}
    </div>
  );
}

function Overview({ spec, groups }: { spec: OpenAPISpec; groups: ApiGroup[] }) {
  const t = useTranslations("apiExplorer");

  return (
    <div className="flex flex-col gap-10">
      <header className="flex flex-col gap-3">
        <h1 className="text-2xl font-semibold">{spec.info?.title}</h1>
        {spec.info?.version ? (
          <p className="text-sm text-fd-muted-foreground">
            {t("version", { version: spec.info.version })}
          </p>
        ) : null}
        {spec.info?.description ? (
          <p className="whitespace-pre-line text-fd-muted-foreground">
            {spec.info.description}
          </p>
        ) : null}
        <p className="text-sm text-fd-muted-foreground">{t("description")}</p>
        <p className="text-sm text-fd-muted-foreground">{t("tokenHint")}</p>
      </header>
      {groups.map((group) => (
        <section key={group.name} className="flex flex-col gap-4">
          <TagDescription group={group} />
          <ul className="divide-y divide-fd-border rounded-lg border border-fd-border">
            {group.operations.map((operation) => (
              <li key={operation.id}>
                <a
                  href={`#${operation.id}`}
                  className="flex flex-col gap-1 px-4 py-3 hover:bg-fd-accent"
                >
                  <span className="flex items-center gap-3">
                    <MethodLabel method={operation.method} />
                    <span className="font-mono text-sm">{operation.path}</span>
                  </span>
                  <span className="text-sm font-medium">{operation.title}</span>
                  {operation.description ? (
                    <span className="whitespace-pre-line text-sm text-fd-muted-foreground">
                      {operation.description}
                    </span>
                  ) : null}
                </a>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function findOperation(groups: ApiGroup[], id: string) {
  for (const group of groups) {
    const operation = group.operations.find((item) => item.id === id);
    if (operation) return { group, operation };
  }
  return undefined;
}

/**
 * Points requests at the device that served the page when the document names
 * no server, so code samples show its address instead of `example.com`.
 */
function withSameOriginServer(spec: OpenAPISpec): OpenAPISpec {
  if (spec.servers?.length) return spec;
  return { ...spec, servers: [{ url: "/" }] };
}

export function ApiExplorer() {
  const t = useTranslations("apiExplorer");
  const [spec, setSpec] = useState<OpenAPISpec | null>(null);
  const [failed, setFailed] = useState(false);
  const groups = useMemo(() => (spec ? groupOperations(spec) : []), [spec]);
  const tree = useMemo(() => buildPageTree(groups), [groups]);
  const fragment = useFragment();
  const selected = findOperation(groups, fragment);

  useEffect(() => {
    const controller = new AbortController();

    fetch(SPEC_URL, { signal: controller.signal })
      .then((response) => {
        if (!response.ok) {
          throw new Error(`GET ${SPEC_URL} returned ${response.status}`);
        }
        return response.json() as Promise<OpenAPISpec>;
      })
      .then((document) => setSpec(withSameOriginServer(document)))
      .catch(() => {
        if (!controller.signal.aborted) {
          setFailed(true);
        }
      });

    return () => controller.abort();
  }, []);

  useEffect(() => {
    window.scrollTo(0, 0);
  }, [fragment]);

  return (
    <DocsLayout
      tree={tree}
      nav={{ title: spec?.info?.title ?? t("title"), url: "#" }}
      slots={{ header: ApiHeader }}
      sidebar={{ components: { Item: SidebarOperation } }}
      containerProps={{ style: HEADER_HEIGHT }}
      themeSwitch={{ enabled: false }}
    >
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-8 px-4 py-8 [grid-area:main]">
        {!spec ? (
          <p className="text-fd-muted-foreground">
            {failed ? t("loadError") : t("loading")}
          </p>
        ) : selected ? (
          <>
            <TagDescription group={selected.group} />
            <OpenAPIPage
              payload={{ bundled: spec }}
              operations={[
                {
                  path: selected.operation.path,
                  method: selected.operation.method,
                },
              ]}
              showTitle
              showDescription
            />
          </>
        ) : (
          <Overview spec={spec} groups={groups} />
        )}
      </div>
    </DocsLayout>
  );
}
