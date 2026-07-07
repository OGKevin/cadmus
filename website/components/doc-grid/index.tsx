"use client";

import { useLocale, useTranslations } from "next-intl";
import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Browsers } from "@phosphor-icons/react/dist/ssr/Browsers";
import { Code } from "@phosphor-icons/react/dist/ssr/Code";
import { Kanban } from "@phosphor-icons/react/dist/ssr/Kanban";
import { DocCard, type DocCardProps } from "../doc-card/index";
import { apiHref, guideHref, storybookHref } from "@/lib/doc-hrefs";

export function DocGrid() {
  const locale = useLocale();
  const t = useTranslations("docs");

  const docs: DocCardProps[] = [
    {
      label: t("userGuide.label"),
      description: t("userGuide.description"),
      href: guideHref(locale),
      icon: BookOpen,
    },
    {
      label: t("translations.label"),
      description: t("translations.description"),
      href: "https://crowdin.com/project/cadmus",
      icon: BookOpen,
    },
    {
      label: t("apiReference.label"),
      description: t("apiReference.description"),
      href: apiHref(locale),
      icon: Code,
    },
    {
      label: t("storybook.label"),
      description: t("storybook.description"),
      href: storybookHref(locale),
      icon: Browsers,
    },
    {
      label: t("planning.label"),
      description: t("planning.description"),
      href: "https://github.com/users/OGKevin/projects/5/views/4",
      icon: Kanban,
    },
  ];

  return (
    <div className="grid w-full max-w-3xl gap-4 sm:grid-cols-3">
      {docs.map((doc) => (
        <DocCard key={doc.href} {...doc} />
      ))}
    </div>
  );
}
