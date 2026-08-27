import type { ComponentProps } from "react";
import {
  Badge as KumoBadge,
  type BadgeVariant,
} from "@cloudflare/kumo/components/badge";

export type BadgeProps = ComponentProps<typeof KumoBadge> & {
  variant?: BadgeVariant;
};

export function Badge(props: BadgeProps) {
  return <KumoBadge variant="secondary" {...props} />;
}
