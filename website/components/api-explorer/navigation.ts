import { useSyncExternalStore } from "react";
import type { OpenAPIPageProps_Spec } from "fumadocs-openapi/ui";

export type OpenAPISpec = NonNullable<
  OpenAPIPageProps_Spec["payload"]["bundled"]
>;

const HTTP_METHODS = [
  "get",
  "put",
  "post",
  "delete",
  "options",
  "head",
  "patch",
  "trace",
] as const;

export type HttpMethod = (typeof HTTP_METHODS)[number];

export interface ApiOperation {
  /** URL fragment that selects this operation. */
  id: string;
  path: string;
  method: HttpMethod;
  title: string;
  description?: string;
}

export interface ApiGroup {
  /** Last segment of the tag name, e.g. "Settings". */
  label: string;
  /** Full tag name, e.g. "cadmus.api.shared.v1.Settings". */
  name: string;
  description?: string;
  operations: ApiOperation[];
}

function operationId(path: string, method: HttpMethod): string {
  const slug = path
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return `${method}-${slug}`;
}

/** Groups every operation under its first tag, in tag declaration order. */
export function groupOperations(spec: OpenAPISpec): ApiGroup[] {
  const groups = new Map<string, ApiGroup>();

  function groupFor(name: string, description?: string): ApiGroup {
    let group = groups.get(name);
    if (!group) {
      group = {
        label: name.split(".").pop() || name,
        name,
        description,
        operations: [],
      };
      groups.set(name, group);
    }
    return group;
  }

  for (const tag of spec.tags ?? []) {
    if (tag.name) groupFor(tag.name, tag.description);
  }

  for (const [path, pathItem] of Object.entries(spec.paths ?? {})) {
    for (const method of HTTP_METHODS) {
      const operation = pathItem?.[method];
      if (!operation) continue;
      groupFor(operation.tags?.[0] ?? "").operations.push({
        id: operationId(path, method),
        path,
        method,
        title: operation.summary ?? path,
        description: operation.description,
      });
    }
  }

  return [...groups.values()].filter((group) => group.operations.length > 0);
}

function subscribeHash(onChange: () => void): () => void {
  window.addEventListener("hashchange", onChange);
  return () => window.removeEventListener("hashchange", onChange);
}

/** The current URL fragment without `#`, e.g. the selected operation id. */
export function useFragment(): string {
  return useSyncExternalStore(
    subscribeHash,
    () => window.location.hash.slice(1),
    () => "",
  );
}
