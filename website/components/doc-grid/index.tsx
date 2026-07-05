import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Browsers } from "@phosphor-icons/react/dist/ssr/Browsers";
import { Code } from "@phosphor-icons/react/dist/ssr/Code";
import { Kanban } from "@phosphor-icons/react/dist/ssr/Kanban";
import { DocCard, type DocCardProps } from "../doc-card/index";

const BASE = process.env.NEXT_PUBLIC_BASE_PATH || "";

const DOCS: DocCardProps[] = [
  {
    label: "User Guide",
    description: "Installation, configuration, and usage",
    href: `${BASE}/guide/`,
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
    href: `${BASE}/api/cadmus_core/`,
    icon: Code,
  },
  {
    label: "Browser Component Library",
    description: "Browse UI components",
    href: `${BASE}/storybook/`,
    icon: Browsers,
  },
  {
    label: "Planning",
    description: "Roadmap, milestones, and active work",
    href: "https://github.com/users/OGKevin/projects/5/views/4",
    icon: Kanban,
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
