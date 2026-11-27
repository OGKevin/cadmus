"use client";

import type { ReactNode } from "react";
import { usePathname } from "@/i18n/navigation";
import { SiteHeader } from "@/components/site-header/index";

export function SiteChrome({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  if (pathname === "/companion" || pathname === "/companion/") {
    return children;
  }

  return (
    <>
      <SiteHeader />
      {children}
    </>
  );
}
