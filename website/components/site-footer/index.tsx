import { Text } from "@cloudflare/kumo/components/text";

export function SiteFooter() {
  return (
    <footer className="border-t border-kumo-line px-6 py-6 text-center">
      <Text variant="secondary" size="sm">
        {new Date().getFullYear()} Cadmus &mdash; powered by{" "}
        <a
          href="https://nextjs.org/"
          target="_blank"
          rel="noopener noreferrer"
          className="text-kumo-link hover:underline"
        >
          Next.js
        </a>
        {""},{" "}
        <a
          href="https://rust-lang.github.io/mdBook/"
          target="_blank"
          rel="noopener noreferrer"
          className="text-kumo-link hover:underline"
        >
          mdBook
        </a>
        {""},{" "}
        <a
          href="https://github.com/cloudflare/kumo"
          target="_blank"
          rel="noopener noreferrer"
          className="text-kumo-link hover:underline"
        >
          kumo
        </a>{" "}
        and{" "}
        <a
          href="https://github.com/storybookjs/storybook"
          target="_blank"
          rel="noopener noreferrer"
          className="text-kumo-link hover:underline"
        >
          storybook.
        </a>
      </Text>
    </footer>
  );
}
