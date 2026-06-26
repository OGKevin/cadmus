import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { Heading } from "./index";
import { Cadmus as CadmusHeading } from "./cadmus";

const meta: Meta<typeof Heading> = {
  title: "Components/Heading",
  component: Heading,
};

export default meta;
type Story = StoryObj<typeof Heading>;

export const Cadmus: Story = {
  render: () => <CadmusHeading />,
};
