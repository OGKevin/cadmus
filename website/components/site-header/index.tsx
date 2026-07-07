import { PageHeader } from "@/components/kumo/page-header/page-header";
import { LanguageSwitcher } from "@/components/language-switcher/index";

export function SiteHeader() {
  return (
    <PageHeader className="w-full" spacing="compact">
      <LanguageSwitcher />
    </PageHeader>
  );
}
