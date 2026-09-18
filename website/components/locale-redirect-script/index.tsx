import { LOCALE_STORAGE_KEY } from "@/i18n/locale-preference";
import { defaultLocale, locales } from "@/i18n/locales.generated";

const basePath = process.env.NEXT_PUBLIC_BASE_PATH || "";

/**
 * Blocking redirect from `/` to `/{locale}/` before paint.
 *
 * Uses `location.replace` so the locale layout (with theme tokens) loads on a
 * full navigation instead of a client-side transition that skips ThemeScript.
 */
export function LocaleRedirectScript() {
  const script = `
(function() {
  var LOCALE_STORAGE_KEY = ${JSON.stringify(LOCALE_STORAGE_KEY)};
  var locales = ${JSON.stringify(locales)};
  var defaultLocale = ${JSON.stringify(defaultLocale)};
  var basePath = ${JSON.stringify(basePath)};
  var stored = null;
  try {
    stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  } catch (e) {}
  var locale = defaultLocale;
  if (stored && locales.indexOf(stored) !== -1) {
    locale = stored;
  } else {
    var browser = (navigator.language || "").split("-")[0];
    if (locales.indexOf(browser) !== -1) {
      locale = browser;
    }
  }
  var target = (basePath || "") + "/" + locale + "/";
  if (window.location.pathname !== target) {
    window.location.replace(target);
  }
})();
`.trim();

  return <script dangerouslySetInnerHTML={{ __html: script }} />;
}
