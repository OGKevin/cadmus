import { LinkButton } from "@cloudflare/kumo/components/button";
import { GithubLogoIcon } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { TagIcon } from "@phosphor-icons/react/dist/ssr/Tag";
import { Actions } from "@/components/actions/index";
import { LATEST_VERSION } from "@/generated/version";

const GITHUB_URL = "https://github.com/ogkevin/cadmus";

export function Github() {
  return (
    <Actions>
      <LinkButton
        href={GITHUB_URL}
        variant="ghost"
        size="lg"
        external
        icon={<GithubLogoIcon weight="fill" />}
      >
        View on GitHub
      </LinkButton>
    </Actions>
  );
}

const RELEASES_URL = `${GITHUB_URL}/releases/latest`;

export function GitHubRelease() {
  return (
    <Actions>
      <LinkButton
        href={RELEASES_URL}
        variant="ghost"
        size="lg"
        external
        icon={<TagIcon weight="fill" />}
      >
        {LATEST_VERSION
          ? `Latest Release (${LATEST_VERSION})`
          : "Latest Release"}
      </LinkButton>
    </Actions>
  );
}
