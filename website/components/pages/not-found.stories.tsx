import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { NotFoundPage } from "./not-found";

const meta: Meta<typeof NotFoundPage> = {
  title: "Pages",
  component: NotFoundPage,
  parameters: { layout: "fullscreen", pageChrome: true },
};

export default meta;
type Story = StoryObj<typeof NotFoundPage>;

export const NotFound: Story = {};
