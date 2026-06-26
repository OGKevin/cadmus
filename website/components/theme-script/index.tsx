/**
 * Blocking inline script injected into <head> before hydration.
 *
 * Sets data-mode on <html> from system prefers-color-scheme to prevent
 * flash-of-wrong-theme. Kumo's light-dark() tokens respond to this attribute.
 */
export function ThemeScript() {
  const script = `
(function() {
  var dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  document.documentElement.dataset.mode = dark ? 'dark' : 'light';
  document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
})();
`.trim();

  return <script dangerouslySetInnerHTML={{ __html: script }} />;
}
