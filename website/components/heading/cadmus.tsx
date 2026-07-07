"use client";

import { useTranslations } from "next-intl";
import { Heading } from ".";

export function Cadmus() {
  const t = useTranslations("heading");

  return <Heading title="Cadmus" subtitle={t("subtitle")} />;
}
