import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { Badge } from "./index";

const meta: Meta<typeof Badge> = {
  title: "Components/Badge",
  component: Badge,
  parameters: { layout: "centered" },
  args: { children: "Label" },
};

export default meta;
type Story = StoryObj<typeof Badge>;

export const Secondary: Story = {};
export const Beta: Story = { args: { variant: "beta", children: "Beta" } };
export const Success: Story = {
  args: { variant: "success", children: "Stable" },
};
export const Error: Story = {
  args: { variant: "error", children: "Deprecated" },
};
export const Info: Story = { args: { variant: "info", children: "Preview" } };
export const Warning: Story = {
  args: { variant: "warning", children: "Experimental" },
};
