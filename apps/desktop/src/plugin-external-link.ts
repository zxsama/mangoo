import type { PluginExternalLink, PluginExternalLinkRequest } from "@mangoo/core-client/node";

export type PluginExternalLinkInput = PluginExternalLinkRequest & { pluginId: string };
export type PluginExternalLinkOpenResult =
  | { ok: true }
  | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" };

export function parsePluginExternalLinkInput(input: unknown): PluginExternalLinkInput | null {
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;
  const record = input as Record<string, unknown>;
  if (Object.keys(record).length !== 3 || Object.keys(record).some((key) => !["pluginId", "kind", "id"].includes(key))) return null;
  if (typeof record.pluginId !== "string" || !/^[a-z0-9][a-z0-9._-]{0,127}$/.test(record.pluginId)) return null;
  if (record.kind !== "authorization" && record.kind !== "help") return null;
  if (typeof record.id !== "string" || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(record.id)) return null;
  return { pluginId: record.pluginId, kind: record.kind, id: record.id };
}

export function isSafePluginExternalUrl(input: unknown, allowLoopback = false): input is string {
  if (typeof input !== "string" || !input || input.length > 8_192 || /[\s\u0000-\u001f\u007f\\#]|%0[0-9a-f]|%1[0-9a-f]|%7f/i.test(input)) return false;
  try {
    const url = new URL(input);
    if (url.username || url.password || url.hash || !url.hostname) return false;
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (loopback) return allowLoopback && ["http:", "https:"].includes(url.protocol);
    return url.protocol === "https:";
  } catch { return false; }
}

/** The renderer supplies an identifier. Only the authenticated Host resolves its destination. */
export async function openPluginExternalLink(
  input: unknown,
  resolve: (pluginId: string, request: PluginExternalLinkRequest) => Promise<PluginExternalLink>,
  opener: (url: string) => Promise<void>,
  options: { allowLoopback?: boolean; isCurrent?: () => boolean; onAuthorizationOpened?: () => void } = {},
): Promise<PluginExternalLinkOpenResult> {
  const request = parsePluginExternalLinkInput(input);
  if (!request) return { ok: false, error: "external_browser_rejected" };
  let link: PluginExternalLink;
  try { link = await resolve(request.pluginId, { kind: request.kind, id: request.id }); }
  catch { return { ok: false, error: "external_browser_rejected" }; }
  if (!link || typeof link !== "object" || link.kind !== request.kind || link.id !== request.id
    || (link.expiresAt !== null && (!Number.isSafeInteger(link.expiresAt) || link.expiresAt <= Date.now()))
    || (request.kind === "authorization" && link.expiresAt === null)
    || !isSafePluginExternalUrl(link.url, options.allowLoopback)
    || options.isCurrent?.() === false) return { ok: false, error: "external_browser_rejected" };
  try {
    await opener(link.url);
    if (request.kind === "authorization" && options.isCurrent?.() !== false) options.onAuthorizationOpened?.();
    return { ok: true };
  } catch { return { ok: false, error: "external_browser_open_failed" }; }
}
