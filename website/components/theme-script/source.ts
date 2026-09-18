/** Inline script run in <head> before paint to set `data-mode` from system theme. */
export const themeScriptSource = `
(function() {
  var dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  document.documentElement.dataset.mode = dark ? 'dark' : 'light';
  document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
})();
`.trim();
