import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const websiteRoot = join(__dirname, "..");
const repoRoot = join(websiteRoot, "..");
const publicDir = join(websiteRoot, "public");
const bundleSource = join(
  websiteRoot,
  "node_modules",
  "@scalar",
  "api-reference",
  "dist",
  "browser",
  "standalone.js",
);
const bundleTarget = join(publicDir, "scalar", "standalone.js");
const BASE_PATH = process.env.NEXT_PUBLIC_BASE_PATH || "";

/** Locale codes from `docs/po/*.po`, always including English. */
function websiteLocales() {
  const codes = new Set(["en"]);
  const poDir = join(repoRoot, "docs", "po");
  if (existsSync(poDir)) {
    for (const file of readdirSync(poDir)) {
      if (file.endsWith(".po")) codes.add(file.replace(/\.po$/, ""));
    }
  }
  return [...codes].sort((left, right) => left.localeCompare(right));
}

function messages(locale) {
  const english = JSON.parse(
    readFileSync(join(websiteRoot, "messages", "en.json"), "utf8"),
  );
  if (locale === "en") return english;
  const local = JSON.parse(
    readFileSync(join(websiteRoot, "messages", `${locale}.json`), "utf8"),
  );
  return {
    ...english,
    ...local,
    apiExplorer: { ...english.apiExplorer, ...local.apiExplorer },
  };
}

function escapeHtml(text) {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function page(locale, copy) {
  return `<!doctype html>
<html lang="${locale}">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(copy.apiExplorer.title)}</title>
    <style>
      html, body { margin: 0; height: 100%; }
      #app { height: 100%; overflow: auto; }
      .agent-button-container,
      [class*="agent-scalar"],
      [class*="scalar-explore-sticker-agent"] { display: none !important; }
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script src="${BASE_PATH}/scalar/standalone.js"></script>
    <script>
      const token = (localStorage.getItem("cadmus-companion-token") ?? "").trim();

      window.Scalar.createApiReference("#app", {
        url: "${BASE_PATH}/openapi.json",
        hideClientButton: true,
        hiddenClients: true,
        hideDownloadButton: true,
        documentDownloadType: "none",
        showDeveloperTools: "never",
        telemetry: false,
        mcp: { disabled: true },
        authentication: token
          ? {
              preferredSecurityScheme: "bearerAuth",
              securitySchemes: { bearerAuth: { token } },
            }
          : undefined,
      });

      const hideAgentButtons = () => {
        for (const button of document.querySelectorAll("button")) {
          const label = button.textContent?.trim();
          if (label === "Ask AI" || label === "Ask AI Agent") {
            button.style.display = "none";
          }
        }
      };
      hideAgentButtons();
      new MutationObserver(hideAgentButtons).observe(document.body, {
        childList: true,
        subtree: true,
      });
    </script>
  </body>
</html>
`;
}

function main() {
  const locales = websiteLocales();
  mkdirSync(join(publicDir, "scalar"), { recursive: true });
  copyFileSync(bundleSource, bundleTarget);

  for (const locale of locales) {
    const target = join(publicDir, locale, "api-explorer");
    mkdirSync(target, { recursive: true });
    writeFileSync(join(target, "index.html"), page(locale, messages(locale)));
  }
}

main();
