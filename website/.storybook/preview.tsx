import { withThemeByDataAttribute } from "@storybook/addon-themes";
import type { Preview, Decorator } from "@storybook/nextjs-vite";
import { SiteHeader } from "../components/site-header/index";
import {
  defaultLocale,
  localeLabels,
  locales,
} from "../i18n/locales.generated";
import nextIntl from "./next-intl";
import "../app/globals.css";

const withPageChrome: Decorator = (Story, context) => {
  if (!context.parameters.pageChrome) {
    return <Story />;
  }

  return (
    <div className="flex min-h-screen flex-col bg-kumo-surface text-kumo-default antialiased">
      <SiteHeader />
      <Story />
    </div>
  );
};

const withPageBackground: Decorator = (Story, context) => {
  if (context.parameters.pageChrome) {
    return <Story />;
  }

  return (
    <div className="bg-kumo-surface min-h-screen p-8">
      <Story />
    </div>
  );
};

const preview: Preview = {
  decorators: [
    withPageChrome,
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
  initialGlobals: {
    locale: defaultLocale,
    locales: Object.fromEntries(
      locales.map((locale) => [locale, localeLabels[locale]]),
    ),
  },
  parameters: {
    nextIntl,
    nextjs: {
      appDirectory: true,
    },
    backgrounds: { disable: true },
  },
};

export default preview;
