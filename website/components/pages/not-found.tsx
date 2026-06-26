import { LinkButton } from "@cloudflare/kumo/components/button";
import { Text } from "@cloudflare/kumo/components/text";

export function NotFoundPage() {
  return (
    <main className="flex flex-1 flex-col items-center justify-center gap-4 px-6 py-24 text-center">
      <Text as="h1" variant="heading1">
        404
      </Text>
      <Text variant="secondary">Page not found.</Text>
      <LinkButton href="/" variant="secondary" size="base">
        Back to home
      </LinkButton>
    </main>
  );
}
