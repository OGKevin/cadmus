import { NextIntlClientProvider, hasLocale } from "next-intl";
import { getMessages, setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";
import { RootProvider } from "fumadocs-ui/provider/next";
import { routing } from "@/i18n/routing";
import "./explorer.css";

export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export default async function ExplorerLocaleLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;

  if (!hasLocale(routing.locales, locale)) {
    notFound();
  }

  setRequestLocale(locale);
  const { apiExplorer, companion, docs, language } = await getMessages();

  return (
    <html lang={locale} suppressHydrationWarning>
      <body className="min-h-screen bg-fd-background text-fd-foreground antialiased">
        <NextIntlClientProvider
          messages={{ apiExplorer, companion, docs, language }}
        >
          <RootProvider
            search={{ enabled: false }}
            theme={{ hotKey: false, attribute: ["class", "data-mode"] }}
          >
            {children}
          </RootProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  );
}
