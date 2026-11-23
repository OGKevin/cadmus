import type { NextConfig } from "next";
import createNextIntlPlugin from "next-intl/plugin";

const withNextIntl = createNextIntlPlugin("./i18n/request.ts");

const devProxy = process.env.NODE_ENV === "development";
const proxyOrigin = process.env.COMPANION_PROXY_ORIGIN;
if (devProxy && !proxyOrigin) {
  throw new Error(
    "COMPANION_PROXY_ORIGIN is not set; run the website with `npm run dev`, which starts scripts/companion-dev-proxy.mjs",
  );
}

const nextConfig: NextConfig = {
  ...(devProxy ? {} : { output: "export" as const }),
  basePath: process.env.NEXT_PUBLIC_BASE_PATH || "",
  trailingSlash: true,
  turbopack: {
    root: __dirname,
  },
  ...(devProxy
    ? {
        skipTrailingSlashRedirect: true,
        async rewrites() {
          return [
            {
              source: "/:locale/guide",
              destination: "/:locale/guide/index.html",
            },
            {
              source: "/:locale/guide/",
              destination: "/:locale/guide/index.html",
            },
            {
              source: "/:locale/api/cadmus_core",
              destination: "/:locale/api/cadmus_core/index.html",
            },
            {
              source: "/:locale/api/cadmus_core/",
              destination: "/:locale/api/cadmus_core/index.html",
            },
            {
              source: "/cadmus.api.:rest(.*)",
              destination: `${proxyOrigin}/cadmus.api.:rest`,
            },
            {
              source: "/openapi.json",
              destination: `${proxyOrigin}/openapi.json`,
            },
          ];
        },
      }
    : {}),
};

export default withNextIntl(nextConfig);
