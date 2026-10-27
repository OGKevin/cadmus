"use client";

import { Text } from "@cloudflare/kumo/components/text";
import { useTranslations } from "next-intl";

export function CompanionInstructions({
  message = "instructions",
}: {
  message?: "instructions" | "down";
}) {
  const t = useTranslations("companion");

  return (
    <section className="flex w-full max-w-3xl flex-col gap-3 rounded-xl border border-kumo-line bg-kumo-base p-6">
      <Text variant="secondary">{t(message)}</Text>
    </section>
  );
}
