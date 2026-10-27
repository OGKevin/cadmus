"use client";

import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import { Text } from "@cloudflare/kumo/components/text";
import { useTranslations } from "next-intl";
import {
  DirectoryBrowser,
  SettingRow,
} from "@/components/companion-setting/index";
import type { Category, DirectoryListing, SettingInput } from "@/lib/companion";

function isLibrariesCategory(category: Category) {
  return (
    category.id === "libraries" ||
    category.settings.some((setting) => setting.kind === "library")
  );
}

export function CategoryList({
  categories,
  pendingId,
  saved,
  onAddLibrary,
  onGetFile,
  onListDirectory,
  onUpdate,
}: {
  categories: Category[];
  pendingId?: string | null;
  saved?: { id: string; tick: number } | null;
  onAddLibrary?: (path: string) => void;
  onGetFile?: (path: string) => Promise<Blob>;
  onListDirectory?: (
    path: string,
    extensions?: string[],
  ) => Promise<DirectoryListing>;
  onUpdate: (id: string, value: SettingInput) => void;
}) {
  const t = useTranslations("companion");
  const [selected, setSelected] = useState(categories[0]?.id ?? "");
  const [adding, setAdding] = useState(false);
  const active =
    categories.find((category) => category.id === selected) ?? categories[0];

  if (!active) {
    return <Text variant="secondary">{t("empty")}</Text>;
  }

  const canAdd = isLibrariesCategory(active) && onAddLibrary && onListDirectory;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="shrink-0 bg-kumo-base pb-6">
        <Tabs
          tabs={categories.map((category) => ({
            value: category.id,
            label: category.label,
          }))}
          value={active.id}
          variant="underline"
          onValueChange={setSelected}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        <div className="flex flex-col gap-3">
          {active.settings.map((setting) => (
            <SettingRow
              key={setting.id}
              pending={pendingId === setting.id}
              savedTick={saved?.id === setting.id ? saved.tick : 0}
              setting={setting}
              onGetFile={onGetFile}
              onListDirectory={onListDirectory}
              onUpdate={(value) => onUpdate(setting.id, value)}
            />
          ))}
          {canAdd ? (
            <Button
              type="button"
              variant="secondary"
              onClick={() => setAdding(true)}
            >
              {t("addLibrary")}
            </Button>
          ) : null}
          {adding && onAddLibrary && onListDirectory ? (
            <DirectoryBrowser
              listDirectory={onListDirectory}
              onClose={() => setAdding(false)}
              onSelect={(path) => {
                setAdding(false);
                onAddLibrary(path);
              }}
            />
          ) : null}
        </div>
      </div>
    </div>
  );
}
