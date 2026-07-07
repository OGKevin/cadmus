"use client";

import { ReactNode } from "react";
import { Tabs, cn, type TabsItem } from "@cloudflare/kumo";

export const KUMO_PAGE_HEADER_VARIANTS = {
  spacing: {
    compact: {
      classes: "gap-1",
      description: "Compact spacing between header elements",
    },
    base: {
      classes: "gap-2",
      description: "Default spacing between header elements",
    },
    relaxed: {
      classes: "gap-4",
      description: "Relaxed spacing for more prominent headers",
    },
  },
} as const;

export const KUMO_PAGE_HEADER_DEFAULT_VARIANTS = {
  spacing: "base",
} as const;

export type KumoPageHeaderSpacing =
  keyof typeof KUMO_PAGE_HEADER_VARIANTS.spacing;

export interface KumoPageHeaderVariantsProps {
  spacing?: KumoPageHeaderSpacing;
}

export function pageHeaderVariants({
  spacing = KUMO_PAGE_HEADER_DEFAULT_VARIANTS.spacing,
}: KumoPageHeaderVariantsProps = {}) {
  return cn(
    "flex flex-col",
    KUMO_PAGE_HEADER_VARIANTS.spacing[spacing].classes,
  );
}

export interface PageHeaderProps extends KumoPageHeaderVariantsProps {
  breadcrumbs?: ReactNode;
  title?: string;
  description?: string;
  tabs?: TabsItem[];
  defaultTab?: string;
  onValueChange?: (value: string) => void;
  className?: string;
  children?: React.ReactNode;
}

export function PageHeader({
  breadcrumbs,
  title,
  description,
  tabs,
  defaultTab,
  onValueChange,
  spacing = "base",
  className,
  children,
}: PageHeaderProps) {
  return (
    <div className={cn(pageHeaderVariants({ spacing }), className)}>
      {breadcrumbs ? (
        <div className="border-b border-kumo-line">{breadcrumbs}</div>
      ) : null}

      {(title || description) && (
        <div className="flex flex-col gap-2 py-3 pl-3">
          {title && (
            <h1 className="font-heading text-3xl font-semibold text-kumo-default">
              {title}
            </h1>
          )}
          {description && (
            <p className="max-w-prose text-base text-kumo-subtle">
              {description}
            </p>
          )}
        </div>
      )}

      {tabs && tabs.length > 0 && (
        <div className="flex w-full items-center justify-between border-b border-kumo-line pt-1 pb-3 pl-3">
          <Tabs
            tabs={tabs}
            selectedValue={defaultTab}
            onValueChange={(nextValue) => {
              onValueChange?.(nextValue);
            }}
          />

          <div className="flex items-center gap-2">{children}</div>
        </div>
      )}

      {(!tabs || tabs.length === 0) && children ? (
        <div className="flex w-full items-center justify-end border-b border-kumo-line px-6 py-3">
          {children}
        </div>
      ) : null}
    </div>
  );
}
