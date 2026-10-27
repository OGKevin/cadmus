"use client";

import { useEffect, useId, useRef, useState } from "react";
import { List } from "@phosphor-icons/react";
import { useLocale, useTranslations } from "next-intl";
import { apiHref, guideHref, rustApiHref } from "@/lib/doc-hrefs";

const menuItemClass =
  "block w-full px-4 py-2 text-left text-sm hover:bg-kumo-tint";

export type DocsMenuProps = {
  /** Highlight the Settings entry as the current page. */
  settingsCurrent?: boolean;
  /** Show the Settings entry after the documentation links. */
  showSettings?: boolean;
  onSettings?: () => void;
};

/** Hamburger menu shared by the companion and explorer pages. */
export function DocsMenu({
  settingsCurrent = false,
  showSettings = false,
  onSettings,
}: DocsMenuProps) {
  const t = useTranslations("companion");
  const docs = useTranslations("docs");
  const locale = useLocale();
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      if (!containerRef.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setOpen(false);
    }
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div ref={containerRef} className="relative">
      <button
        type="button"
        className="inline-flex items-center rounded-lg border border-kumo-line bg-kumo-base p-2.5 text-kumo-default transition-colors hover:border-kumo-focus/40 hover:bg-kumo-tint"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={menuId}
        aria-label={t("menu")}
        onClick={() => setOpen((value) => !value)}
      >
        <List size={20} weight="duotone" aria-hidden />
      </button>
      {open ? (
        <ul
          id={menuId}
          role="menu"
          aria-label={t("menu")}
          className="absolute left-0 z-50 mt-2 min-w-44 overflow-hidden rounded-lg border border-kumo-line bg-kumo-base py-1 shadow-lg"
        >
          <li role="none">
            <a
              href={guideHref(locale)}
              role="menuitem"
              className={menuItemClass}
            >
              {docs("userGuide.label")}
            </a>
          </li>
          <li role="none">
            <a href={apiHref(locale)} role="menuitem" className={menuItemClass}>
              {docs("apiReference.label")}
            </a>
          </li>
          <li role="none">
            <a
              href={rustApiHref(locale)}
              role="menuitem"
              className={menuItemClass}
            >
              {docs("rustApi.label")}
            </a>
          </li>
          {showSettings && onSettings ? (
            <>
              <li role="separator" className="my-1 border-t border-kumo-line" />
              <li role="none">
                <button
                  type="button"
                  role="menuitem"
                  className={menuItemClass}
                  aria-current={settingsCurrent ? "page" : undefined}
                  onClick={() => {
                    onSettings();
                    setOpen(false);
                  }}
                >
                  {t("settings")}
                </button>
              </li>
            </>
          ) : null}
        </ul>
      ) : null}
    </div>
  );
}
