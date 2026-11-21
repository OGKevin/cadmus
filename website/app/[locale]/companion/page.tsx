import { Suspense } from "react";
import { setRequestLocale } from "next-intl/server";
import { CompanionPage, CompanionQuery } from "@/components/pages/companion";
import { routing } from "@/i18n/routing";

export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export default async function CompanionRoute({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  setRequestLocale(locale);

  return (
    <Suspense fallback={<CompanionPage />}>
      <CompanionQuery />
    </Suspense>
  );
}
