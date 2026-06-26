import { withThemeByDataAttribute } from "@storybook/addon-themes";
import type { Preview, Decorator } from "@storybook/nextjs-vite";
import "../app/globals.css";

const withPageBackground: Decorator = (Story) => (
  <div className="bg-kumo-surface min-h-screen p-8">
    <Story />
  </div>
);

const preview: Preview = {
  decorators: [
    withThemeByDataAttribute({
      themes: {
        light: "light",
        dark: "dark",
      },
      defaultTheme: "light",
      attributeName: "data-mode",
    }),
    withPageBackground,
  ],
  parameters: {
    nextjs: {
      appDirectory: true,
    },
    backgrounds: { disable: true },
  },
};

export default preview;
