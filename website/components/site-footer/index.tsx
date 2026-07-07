"use client";

import { Text } from "@cloudflare/kumo/components/text";
import { useTranslations } from "next-intl";

export function SiteFooter() {
  const t = useTranslations("footer");
  const year = new Date().getFullYear();

  return (
    <footer className="border-t border-kumo-line px-6 py-6 text-center">
      <Text variant="secondary" size="sm">
        {t.rich("poweredBy", {
          year,
          nextjs: (chunks) => (
            <a
              href="https://nextjs.org/"
              target="_blank"
              rel="noopener noreferrer"
              className="text-kumo-link hover:underline"
            >
              {chunks}
            </a>
          ),
          mdbook: (chunks) => (
            <a
              href="https://rust-lang.github.io/mdBook/"
              target="_blank"
              rel="noopener noreferrer"
              className="text-kumo-link hover:underline"
            >
              {chunks}
            </a>
          ),
          kumo: (chunks) => (
            <a
              href="https://github.com/cloudflare/kumo"
              target="_blank"
              rel="noopener noreferrer"
              className="text-kumo-link hover:underline"
            >
              {chunks}
            </a>
          ),
          storybook: (chunks) => (
            <a
              href="https://github.com/storybookjs/storybook"
              target="_blank"
              rel="noopener noreferrer"
              className="text-kumo-link hover:underline"
            >
              {chunks}
            </a>
          ),
        })}
      </Text>
    </footer>
  );
}
