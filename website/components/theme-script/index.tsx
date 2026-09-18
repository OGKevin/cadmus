import { themeScriptSource } from "./source";

/**
 * Blocking inline script injected into <head> before hydration.
 *
 * Sets data-mode on <html> from system prefers-color-scheme to prevent
 * flash-of-wrong-theme. Kumo's light-dark() tokens respond to this attribute.
 */
export function ThemeScript() {
  return <script dangerouslySetInnerHTML={{ __html: themeScriptSource }} />;
}
