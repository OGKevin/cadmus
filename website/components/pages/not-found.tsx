"use client";

import { LinkButton } from "@cloudflare/kumo/components/button";
import { Text } from "@cloudflare/kumo/components/text";
import { useLocale, useTranslations } from "next-intl";
import { homeHref } from "@/lib/doc-hrefs";

export function NotFoundPage() {
  const locale = useLocale();
  const t = useTranslations("notFound");

  return (
    <main className="flex flex-1 flex-col items-center justify-center gap-4 px-6 py-24 text-center">
      <Text as="h1" variant="heading1">
        404
      </Text>
      <Text variant="secondary">{t("message")}</Text>
      <LinkButton href={homeHref(locale)} variant="secondary" size="base">
        {t("backHome")}
      </LinkButton>
    </main>
  );
}
