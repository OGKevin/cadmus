import { DocGrid } from "@/components/doc-grid/index";
import { SiteFooter } from "@/components/site-footer/index";
import { Github } from "@/components/actions/github";
import { Cadmus } from "@/components/heading/cadmus";
import { Discord } from "@/components/actions/discord";
import { GitHubRelease } from "@/components/actions/github";

export function LandingPage() {
  return (
    <>
      <main className="flex flex-1 flex-col items-center justify-center gap-16 px-6 py-24">
        <div className="flex flex-col items-center gap-6">
          <Cadmus />
          <div className="flex flex-row flex-wrap items-center justify-center gap-3">
            <Github />
            <Discord />
            <GitHubRelease />
          </div>
        </div>
        <DocGrid />
      </main>
      <SiteFooter />
    </>
  );
}
