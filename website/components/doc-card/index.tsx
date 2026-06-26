import { Text } from "@cloudflare/kumo/components/text";
import type { Icon } from "@phosphor-icons/react";

export interface DocCardProps {
  label: string;
  description: string;
  href: string;
  icon: Icon;
}

export function DocCard({
  label,
  description,
  href,
  icon: CardIcon,
}: DocCardProps) {
  return (
    <a
      href={href}
      className="flex flex-col gap-2 rounded-xl border border-kumo-line bg-kumo-base p-6 transition-colors hover:border-kumo-focus/40 hover:bg-kumo-tint"
    >
      <div className="flex items-center gap-2">
        <CardIcon size={18} className="text-kumo-link" weight="duotone" />
        <Text as="span" variant="heading3">
          {label}
        </Text>
      </div>
      <Text variant="secondary" size="sm">
        {description}
      </Text>
    </a>
  );
}
