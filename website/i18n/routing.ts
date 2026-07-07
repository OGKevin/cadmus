import { defineRouting } from "next-intl/routing";
import { defaultLocale, locales } from "./locales.generated";

export const routing = defineRouting({
  locales,
  defaultLocale,
  localePrefix: "always",
});
