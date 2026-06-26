import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { LandingPage } from "./landing";

const meta: Meta<typeof LandingPage> = {
  title: "Pages",
  component: LandingPage,
  parameters: { layout: "fullscreen" },
};

export default meta;
type Story = StoryObj<typeof LandingPage>;

export const Landing: Story = {};
