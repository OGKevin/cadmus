import type { StorybookConfig } from "@storybook/nextjs-vite";
import tailwindcss from "@tailwindcss/vite";
import path from "path";
import { fileURLToPath } from "url";
import type { InlineConfig, Plugin } from "vite";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const config: StorybookConfig = {
  stories: ["../components/**/*.stories.@(ts|tsx)"],
  addons: ["@storybook/addon-themes", "storybook-next-intl"],
  framework: {
    name: "@storybook/nextjs-vite",
    options: {
      nextConfigPath: path.resolve(__dirname, "../next.config.ts"),
    },
  },
  viteFinal: (viteConfig): InlineConfig => ({
    ...viteConfig,
    base: "./",
    publicDir: false,
    plugins: [
      ...(viteConfig.plugins ?? []),
      tailwindcss(),
      next16LayoutRouter(),
    ],
  }),
};

function next16LayoutRouter(): Plugin {
  return {
    name: "next-16-storybook-layout-router",
    transform(code: string, id: string) {
      if (
        !id.includes("@storybook/nextjs-vite") ||
        !code.includes("parentCacheNode:") ||
        code.includes("parentRenderTree:")
      ) {
        return;
      }
      return code.replace(
        "parentCacheNode: newLazyCacheNode,",
        "parentCacheNode: newLazyCacheNode, parentRenderTree: { data: { bfcacheId: 0 } },",
      );
    },
  };
}

export default config;
