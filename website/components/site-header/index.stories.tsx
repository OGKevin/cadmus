import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { SiteHeader } from "./index";

const meta: Meta<typeof SiteHeader> = {
  title: "Components/SiteHeader",
  component: SiteHeader,
  parameters: {
    layout: "fullscreen",
    nextjs: {
      appDirectory: true,
      navigation: {
        pathname: "/",
      },
    },
  },
};

export default meta;

type Story = StoryObj<typeof SiteHeader>;

export const Default: Story = {};
