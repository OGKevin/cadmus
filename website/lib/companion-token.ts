/** localStorage key holding the companion bearer token, shared by the companion and API explorer pages. */
export const TOKEN_KEY = "cadmus-companion-token";

/** sessionStorage key recording the token last copied into the API playground in this tab. */
export const PREFILL_KEY = "cadmus-companion-token-prefill";

/**
 * Decides whether the playground's session value is replaced with the saved
 * companion token. Blank values and the token copied by an earlier prefill are
 * stale; anything else was typed by the user in this tab and is kept.
 */
export function shouldReplaceSessionToken(
  current: unknown,
  lastPrefill: string | null,
): boolean {
  if (typeof current !== "string" || !current.trim()) return true;
  return current === lastPrefill;
}
