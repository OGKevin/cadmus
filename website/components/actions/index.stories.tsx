import { LinkButton } from "@cloudflare/kumo/components/button";
import type { Meta, StoryObj } from "@storybook/nextjs-vite";
import { Actions } from "./index";
import {
  Github as GithubAction,
  GitHubRelease as GithubReleaseAction,
} from "./github";
import { GithubLogoIcon } from "@phosphor-icons/react";
import { Discord as DiscordAction } from "./discord";

const meta: Meta<typeof Actions> = {
  title: "Components/Actions",
  component: Actions,
};

export default meta;
type Story = StoryObj<typeof Actions>;

export const Github: Story = {
  render: () => <GithubAction />,
};

export const Generic: Story = {
  args: {
    children: (
      <LinkButton
        href="https://github.com"
        variant="ghost"
        size="lg"
        external
        icon={<GithubLogoIcon weight="fill" />}
      >
        View on GitHub
      </LinkButton>
    ),
  },
};

export const Discord: Story = {
  render: () => <DiscordAction />,
};

export const GithubRelease: Story = {
  render: () => <GithubReleaseAction />,
};
