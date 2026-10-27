import { useRef, useState } from "react";
import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import {
  applySetting,
  type Category,
  type DirectoryListing,
} from "@/lib/companion";
import { CategoryList } from "./index";

async function mockListDirectory(path: string): Promise<DirectoryListing> {
  const root = "/mnt/onboard";
  if (!path || path === root) {
    return {
      path: root,
      parent: null,
      entries: [
        { name: "books", path: `${root}/books`, directory: true },
        { name: "docs", path: `${root}/docs`, directory: true },
      ],
    };
  }
  return { path, parent: root, entries: [] };
}

const fixture: Category[] = [
  {
    id: "reading",
    label: "Reading",
    settings: [
      {
        id: "page-turn",
        protoId: { kind: "SETTING_IDENTITY_KIND_PAGE_TURN" },
        label: "Show page turns",
        kind: "toggle",
        toggle: true,
      },
      {
        id: "theme",
        protoId: { kind: "SETTING_IDENTITY_KIND_THEME" },
        label: "Theme",
        kind: "choice",
        choice: {
          selected: "light",
          options: [
            { id: "light", label: "Light" },
            { id: "dark", label: "Dark" },
          ],
        },
      },
    ],
  },
  {
    id: "display",
    label: "Display",
    settings: [
      {
        id: "frontlight",
        protoId: { kind: "SETTING_IDENTITY_KIND_FRONTLIGHT" },
        label: "Frontlight",
        kind: "toggle",
        toggle: false,
      },
    ],
  },
];

function List() {
  const [categories, setCategories] = useState(fixture);
  return (
    <div className="w-full max-w-xl">
      <CategoryList
        categories={categories}
        onUpdate={(id, value) =>
          setCategories((current) => applySetting(current, id, value))
        }
      />
    </div>
  );
}

const meta: Meta<typeof CategoryList> = {
  title: "Components/CompanionCategories",
  component: CategoryList,
  parameters: {
    nextjs: { appDirectory: false },
  },
};

export default meta;
type Story = StoryObj<typeof CategoryList>;

export const Reading: Story = {
  render: () => <List />,
};

const librariesFixture: Category[] = [
  {
    id: "libraries",
    label: "Libraries",
    settings: [
      {
        id: "Library(0)",
        protoId: {
          kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
          index: 0,
          key: "/mnt/onboard/books",
        },
        label: "Library",
        kind: "library",
        library: {
          name: "Books",
          path: { path: "/mnt/onboard/books" },
          finished: {
            selected: "close",
            options: [
              { id: "close", label: "Close" },
              { id: "next", label: "Go to next" },
            ],
          },
        },
      },
    ],
  },
];

function LibrariesList() {
  const [categories, setCategories] = useState(librariesFixture);
  const nextIndex = useRef(
    librariesFixture.flatMap((category) => category.settings).length,
  );
  const active = categories[0];
  if (!active) return null;
  return (
    <div className="w-full max-w-xl">
      <CategoryList
        categories={categories}
        onAddLibrary={(path) => {
          const index = nextIndex.current++;
          setCategories([
            {
              ...active,
              settings: [
                ...active.settings,
                {
                  id: `Library(${index})`,
                  protoId: {
                    kind: "SETTING_IDENTITY_KIND_LIBRARY_INFO",
                    index,
                  },
                  label: "Library",
                  kind: "library",
                  library: {
                    name: "",
                    path: { path },
                    finished: {
                      selected: "close",
                      options: [
                        { id: "close", label: "Close" },
                        { id: "next", label: "Go to next" },
                      ],
                    },
                  },
                },
              ],
            },
          ]);
        }}
        onListDirectory={mockListDirectory}
        onUpdate={(id, value) =>
          setCategories((current) => applySetting(current, id, value))
        }
      />
    </div>
  );
}

export const Libraries: Story = {
  render: () => <LibrariesList />,
};
