import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { DocCard } from "./index";
import { BookOpenIcon } from "@phosphor-icons/react";

const meta: Meta<typeof DocCard> = {
  title: "Components/DocCard",
  component: DocCard,
  parameters: {
    layout: "centered",
  },
  args: {
    label: "User Guide",
    description: "Installation, configuration, and usage",
    href: "/guide/",
    icon: BookOpenIcon,
  },
};

export default meta;

type Story = StoryObj<typeof DocCard>;

export const UserGuide: Story = {};
