import { LocaleRedirectScript } from "@/components/locale-redirect-script/index";
import { ThemeScript } from "@/components/theme-script/index";
import { defaultLocale } from "@/i18n/locales.generated";

export default function RootRedirectPage() {
  return (
    <html lang={defaultLocale} suppressHydrationWarning>
      <head>
        <ThemeScript />
        <LocaleRedirectScript />
      </head>
      <body />
    </html>
  );
}
