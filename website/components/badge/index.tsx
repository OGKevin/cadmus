import type { ComponentProps } from "react";
import {
  Badge as KumoBadge,
  type BadgeVariant,
} from "@cloudflare/kumo/components/badge";

export interface BadgeProps extends ComponentProps<typeof KumoBadge> {
  variant?: BadgeVariant;
}

export function Badge({ variant = "secondary", ...props }: BadgeProps) {
  return <KumoBadge variant={variant} {...props} />;
}
