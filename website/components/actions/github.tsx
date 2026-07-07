"use client";

import { LinkButton } from "@cloudflare/kumo/components/button";
import { GithubLogoIcon } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { TagIcon } from "@phosphor-icons/react/dist/ssr/Tag";
import { useTranslations } from "next-intl";
import { Actions } from "@/components/actions/index";
import { LATEST_VERSION } from "@/generated/version";

const GITHUB_URL = "https://github.com/ogkevin/cadmus";

export function Github() {
  const t = useTranslations("actions");

  return (
    <Actions>
      <LinkButton
        href={GITHUB_URL}
        variant="ghost"
        size="lg"
        external
        icon={<GithubLogoIcon weight="fill" />}
      >
        {t("viewOnGithub")}
      </LinkButton>
    </Actions>
  );
}

const RELEASES_URL = `${GITHUB_URL}/releases/latest`;

export function GitHubRelease() {
  const t = useTranslations("actions");

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
          ? t("latestReleaseVersion", { version: LATEST_VERSION })
          : t("latestRelease")}
      </LinkButton>
    </Actions>
  );
}
