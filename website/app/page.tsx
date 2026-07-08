"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { LOCALE_STORAGE_KEY } from "@/i18n/locale-preference";
import { defaultLocale, locales } from "@/i18n/locales.generated";

function resolveLocale(): string {
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  } catch {
    // localStorage unavailable; fall through to navigator.language
  }
  if (stored && (locales as readonly string[]).includes(stored)) {
    return stored;
  }

  const browser = navigator.language.split("-")[0];
  if ((locales as readonly string[]).includes(browser)) {
    return browser;
  }

  return defaultLocale;
}

export default function RootRedirectPage() {
  const router = useRouter();

  useEffect(() => {
    router.replace(`/${resolveLocale()}/`);
  }, [router]);

  return (
    <html lang={defaultLocale}>
      <body />
    </html>
  );
}
