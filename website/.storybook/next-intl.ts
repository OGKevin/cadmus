/// <reference types="vite/client" />

import enMessages from "../messages/en.json";
import { defaultLocale, locales } from "../i18n/locales.generated";

type Messages = Record<string, unknown>;

function mergeMessages(base: Messages, override: Messages): Messages {
  const result: Messages = { ...base };
  for (const [key, value] of Object.entries(override)) {
    const baseValue = base[key];
    if (
      value !== null &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      baseValue !== null &&
      typeof baseValue === "object" &&
      !Array.isArray(baseValue)
    ) {
      result[key] = mergeMessages(baseValue as Messages, value as Messages);
    } else {
      result[key] = value;
    }
  }
  return result;
}

const messageModules = import.meta.glob("../messages/*.json", {
  eager: true,
}) as Record<string, { default: Messages }>;

const messagesByLocale = Object.fromEntries(
  locales.map((locale) => {
    if (locale === defaultLocale) {
      return [locale, enMessages];
    }

    const messageModule = messageModules[`../messages/${locale}.json`];
    const localeMessages = messageModule?.default ?? {};
    return [locale, mergeMessages(enMessages, localeMessages)];
  }),
);

const nextIntl = {
  defaultLocale,
  messagesByLocale,
};

export default nextIntl;
