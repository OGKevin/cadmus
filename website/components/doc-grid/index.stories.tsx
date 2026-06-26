import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { DocGrid } from "./index";

const meta: Meta<typeof DocGrid> = {
  title: "Components/DocGrid",
  component: DocGrid,
  parameters: {
    layout: "centered",
  },
};

export default meta;

type Story = StoryObj<typeof DocGrid>;

export const Default: Story = {};
