import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { LanguageSwitcher } from "./index";

const meta: Meta<typeof LanguageSwitcher> = {
  title: "Components/LanguageSwitcher",
  component: LanguageSwitcher,
  parameters: {
    layout: "centered",
    nextjs: {
      appDirectory: true,
      navigation: {
        pathname: "/",
      },
    },
  },
};

export default meta;

type Story = StoryObj<typeof LanguageSwitcher>;

export const Default: Story = {};
