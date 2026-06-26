import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Browsers } from "@phosphor-icons/react/dist/ssr/Browsers";
import { Code } from "@phosphor-icons/react/dist/ssr/Code";
import { DocCard, type DocCardProps } from "../doc-card/index";

const DOCS: DocCardProps[] = [
  {
    label: "User Guide",
    description: "Installation, configuration, and usage",
    href: "/guide/",
    icon: BookOpen,
  },
  {
    label: "Translations",
    description: "Help translate Cadmus",
    href: "https://crowdin.com/project/cadmus",
    icon: BookOpen,
  },
  {
    label: "API Reference",
    description: "Rust crate documentation",
    href: "/api/cadmus_core/",
    icon: Code,
  },
  {
    label: "Browser Component Library",
    description: "Browse UI components",
    href: "/storybook/",
    icon: Browsers,
  },
];

export function DocGrid() {
  return (
    <div className="grid w-full max-w-3xl gap-4 sm:grid-cols-3">
      {DOCS.map((doc) => (
        <DocCard key={doc.href} {...doc} />
      ))}
    </div>
  );
}
