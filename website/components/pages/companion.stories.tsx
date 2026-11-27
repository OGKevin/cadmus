import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import type { Category } from "@/lib/companion";
import { CompanionPage } from "./companion";

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
      {
        id: "device-name",
        protoId: { kind: "SETTING_IDENTITY_KIND_DEVICE_NAME" },
        label: "Device name",
        kind: "text",
        mode: "text",
        text: "Kobo",
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

const meta: Meta<typeof CompanionPage> = {
  title: "Pages/Companion",
  component: CompanionPage,
  parameters: { layout: "fullscreen", pageChrome: true, siteHeader: false },
};

export default meta;
type Story = StoryObj<typeof CompanionPage>;

export const Instructions: Story = {};

export const Home: Story = {
  args: {
    token: "123456",
    status: {
      version: "v0.12.0",
      hostname: "cadmus-01a15843796a7710ada3dcc67bba96aa",
      locale: "en",
    },
  },
};

export const Editor: Story = {
  args: { categories: fixture, view: "settings" },
};
