import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { CompanionInstructions } from "./index";

const meta: Meta<typeof CompanionInstructions> = {
  title: "Components/CompanionInstructions",
  component: CompanionInstructions,
};

export default meta;
type Story = StoryObj<typeof CompanionInstructions>;

export const Panel: Story = {};
