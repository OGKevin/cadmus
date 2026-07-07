"use client";

import { Globe } from "@phosphor-icons/react";
import { useLocale, useTranslations } from "next-intl";
import { useEffect, useId, useRef, useState } from "react";
import { Link, usePathname } from "@/i18n/navigation";
import { LOCALE_STORAGE_KEY } from "@/i18n/locale-preference";
import {
  defaultLocale,
  localeLabels,
  locales,
  type Locale,
} from "@/i18n/locales.generated";

function persistLocale(locale: Locale) {
  try {
    if (typeof window === "undefined" || !window.localStorage) return;
    localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch (error) {
    console.warn("Failed to persist locale preference", error);
  }
}

export function LanguageSwitcher() {
  const t = useTranslations("language");
  const activeLocale = useLocale() as Locale;
  const pathname = usePathname();
  const menuId = useId();
  const containerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    function handlePointerDown(event: MouseEvent) {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        setOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    }

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  return (
    <div ref={containerRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        className="inline-flex items-center gap-2 rounded-lg border border-kumo-line bg-kumo-base px-3 py-2.5 text-kumo-default transition-colors hover:border-kumo-focus/40 hover:bg-kumo-tint"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={menuId}
        aria-label={t("change")}
        onClick={() => setOpen((value) => !value)}
      >
        <Globe size={20} weight="duotone" aria-hidden />
        <span className="text-sm font-medium">
          {activeLocale.toUpperCase()}
        </span>
      </button>
      {open ? (
        <ul
          id={menuId}
          role="menu"
          aria-label={t("change")}
          className="absolute right-0 z-10 mt-2 min-w-40 overflow-hidden rounded-lg border border-kumo-line bg-kumo-base py-1 shadow-lg"
        >
          {locales.map((locale) => {
            const selected = locale === activeLocale;

            return (
              <li key={locale} role="none">
                <Link
                  href={pathname}
                  locale={locale}
                  role="menuitem"
                  aria-current={selected ? "true" : undefined}
                  className={`block px-4 py-2 text-sm transition-colors hover:bg-kumo-tint ${
                    selected
                      ? "bg-kumo-tint font-medium text-kumo-link"
                      : "text-kumo-default"
                  }`}
                  onClick={() => {
                    persistLocale(locale);
                    setOpen(false);
                  }}
                >
                  {localeLabels[locale]}
                </Link>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

export function resolveStoredLocale(): Locale {
  if (typeof window === "undefined") {
    return defaultLocale;
  }

  let stored: string | null = null;
  try {
    stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  } catch {
    return defaultLocale;
  }
  if (stored && (locales as readonly string[]).includes(stored)) {
    return stored as Locale;
  }

  return defaultLocale;
}
