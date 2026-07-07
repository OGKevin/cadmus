const basePath = process.env.NEXT_PUBLIC_BASE_PATH || "";

export function homeHref(locale: string) {
  return `${basePath}/${locale}/`;
}

export function guideHref(locale: string) {
  return `${basePath}/${locale}/guide/`;
}

export function apiHref(locale: string) {
  return `${basePath}/${locale}/api/cadmus_core/`;
}

export function storybookHref(locale: string) {
  return `${basePath}/${locale}/storybook/`;
}
