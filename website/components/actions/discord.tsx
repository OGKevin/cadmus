import { LinkButton } from "@cloudflare/kumo/components/button";
import { DiscordLogoIcon } from "@phosphor-icons/react/dist/ssr/DiscordLogo";
import { Actions } from "@/components/actions/index";

const DISCORD_URL = "https://discord.gg/3AJHp6rV5a";

export function Discord() {
  return (
    <Actions>
      <LinkButton
        href={DISCORD_URL}
        variant="ghost"
        size="lg"
        external
        icon={<DiscordLogoIcon weight="fill" />}
      >
        Discord
      </LinkButton>
    </Actions>
  );
}
