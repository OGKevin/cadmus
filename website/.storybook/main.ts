import type { StorybookConfig } from "@storybook/nextjs-vite";
import tailwindcss from "@tailwindcss/vite";
import path from "path";
import { fileURLToPath } from "url";
import type { InlineConfig } from "vite";

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
    plugins: [...(viteConfig.plugins ?? []), tailwindcss()],
  }),
};

export default config;
