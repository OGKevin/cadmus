import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import {
  isLibraryRemove,
  withSettingValue,
  type DirectoryListing,
  type Setting,
} from "@/lib/companion";
import { DirectoryBrowser, SettingRow } from "./index";

const PIXEL_PNG = (() => {
  const binary = atob(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  );
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new Blob([bytes.buffer], { type: "image/png" });
})();

async function mockListDirectory(path: string): Promise<DirectoryListing> {
  const root = "/mnt/onboard";
  const listings: Record<string, DirectoryListing> = {
    [root]: {
      path: root,
      parent: null,
      entries: [
        { name: "books", path: `${root}/books`, directory: true },
        { name: "cover.png", path: `${root}/cover.png`, directory: false },
        { name: "docs", path: `${root}/docs`, directory: true },
      ],
    },
    [`${root}/books`]: {
      path: `${root}/books`,
      parent: root,
      entries: [
        {
          name: "fiction",
          path: `${root}/books/fiction`,
          directory: true,
        },
      ],
    },
    [`${root}/books/fiction`]: {
      path: `${root}/books/fiction`,
      parent: `${root}/books`,
      entries: [],
    },
    [`${root}/docs`]: {
      path: `${root}/docs`,
      parent: root,
      entries: [],
    },
  };
  const requested = path || root;
  return listings[requested] ?? { path: requested, parent: root, entries: [] };
}

async function mockGetFile(): Promise<Blob> {
  return PIXEL_PNG;
}

function Row({ setting }: { setting: Setting }) {
  const [current, setCurrent] = useState(setting);
  return (
    <div className="w-full max-w-xl">
      <SettingRow
        setting={current}
        onGetFile={mockGetFile}
        onListDirectory={mockListDirectory}
        onUpdate={(value) => {
          if (isLibraryRemove(value)) return;
          setCurrent(withSettingValue(current, value));
        }}
      />
    </div>
  );
}

const toggle: Setting = {
  id: "page-turn",
  protoId: { kind: "SETTING_IDENTITY_KIND_PAGE_TURN" },
  label: "Show page turns",
  kind: "toggle",
  toggle: true,
};

const choice: Setting = {
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
};

const text: Setting = {
  id: "device-name",
  protoId: { kind: "SETTING_IDENTITY_KIND_DEVICE_NAME" },
  label: "Device name",
  kind: "text",
  mode: "text",
  text: "Kobo",
};

const integer: Setting = {
  id: "retention",
  protoId: { kind: "SETTING_IDENTITY_KIND_RETENTION" },
  label: "Settings to keep",
  kind: "text",
  mode: "integer",
  text: "3",
};

const number: Setting = {
  id: "idle",
  protoId: { kind: "SETTING_IDENTITY_KIND_IDLE" },
  label: "Wi-Fi idle",
  kind: "text",
  mode: "number",
  text: "5.0",
};

const hostname: Setting = {
  id: "NtpServer",
  protoId: { kind: "SETTING_IDENTITY_KIND_NTP_SERVER" },
  label: "NTP server",
  kind: "text",
  mode: "hostname",
  text: "time.cloudflare.com",
};

const intermission: Setting = {
  id: "IntermissionSuspend",
  protoId: { kind: "SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND" },
  label: "Suspend screen",
  kind: "choice",
  choice: {
    selected: "/mnt/onboard/cover.png",
    options: [
      { id: "Logo", label: "Logo" },
      { id: "Cover", label: "Cover" },
      { id: "Blank", label: "Blank" },
      { id: "/mnt/onboard/cover.png", label: "cover.png" },
    ],
  },
};

const fillColor: Setting = {
  id: "IntermissionFillColor",
  protoId: { kind: "SETTING_IDENTITY_KIND_INTERMISSION_FILL_COLOR" },
  label: "Fill Color",
  kind: "choice",
  choice: {
    selected: "#804020",
    options: [
      { id: "#ffffff", label: "White" },
      { id: "#000000", label: "Black" },
      { id: "#aa0000", label: "#aa0000" },
      { id: "#00aa00", label: "#00aa00" },
      { id: "#804020", label: "#804020" },
    ],
  },
};

const checks: Setting = {
  id: "DitheredKinds",
  protoId: { kind: "SETTING_IDENTITY_KIND_DITHERED_KINDS" },
  label: "Dithered File Types",
  kind: "checks",
  items: [
    { id: "epub", label: "EPUB", on: true },
    { id: "pdf", label: "PDF", on: false },
    { id: "cbz", label: "CBZ", on: true },
    { id: "png", label: "PNG", on: false },
  ],
};

const keyboard: Setting = {
  id: "KeyboardLayout",
  protoId: { kind: "SETTING_IDENTITY_KIND_KEYBOARD_LAYOUT" },
  label: "Keyboard",
  kind: "choice",
  choice: {
    selected: "English",
    options: [
      { id: "English", label: "English" },
      { id: "German", label: "German" },
    ],
  },
};

const library: Setting = {
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
};

const refreshRate: Setting = {
  id: "RefreshRate",
  protoId: { kind: "SETTING_IDENTITY_KIND_REFRESH_RATE" },
  label: "Refresh Rate",
  kind: "refreshRate",
  refreshRate: {
    regular: 8,
    inverted: 2,
    byKind: [{ extension: "EXTENSION_EPUB", regular: 12, inverted: 4 }],
    available: ["EXTENSION_CBZ", "EXTENSION_PDF"],
  },
};

const meta: Meta<typeof SettingRow> = {
  title: "Components/CompanionSetting",
  component: SettingRow,
  parameters: {
    nextjs: { appDirectory: false },
  },
};

export default meta;
type Story = StoryObj<typeof SettingRow>;

export const Toggle: Story = {
  render: () => <Row setting={toggle} />,
};

export const Choice: Story = {
  render: () => <Row setting={choice} />,
};

export const Text: Story = {
  render: () => <Row setting={text} />,
};

export const Integer: Story = {
  render: () => <Row setting={integer} />,
};

export const Number: Story = {
  render: () => <Row setting={number} />,
};

export const Hostname: Story = {
  render: () => <Row setting={hostname} />,
};

export const FillColor: Story = {
  render: () => <Row setting={fillColor} />,
};

export const IntermissionImage: Story = {
  render: () => <Row setting={intermission} />,
};

export const RefreshRate: Story = {
  render: () => <Row setting={refreshRate} />,
};

export const Checks: Story = {
  render: () => <Row setting={checks} />,
};

export const ChoiceEnglish: Story = {
  render: () => <Row setting={keyboard} />,
};

export const Library: Story = {
  render: () => <Row setting={library} />,
};

export const Directory: Story = {
  render: () => (
    <DirectoryBrowser
      listDirectory={mockListDirectory}
      onClose={() => undefined}
      onSelect={() => undefined}
    />
  ),
};

export const FileBrowser: Story = {
  render: () => (
    <DirectoryBrowser
      extensions={[
        "EXTENSION_PNG",
        "EXTENSION_JPG",
        "EXTENSION_JPEG",
        "EXTENSION_WEBP",
        "EXTENSION_SVG",
      ]}
      listDirectory={mockListDirectory}
      mode="file"
      onClose={() => undefined}
      onGetFile={mockGetFile}
      onSelect={() => undefined}
    />
  ),
};
