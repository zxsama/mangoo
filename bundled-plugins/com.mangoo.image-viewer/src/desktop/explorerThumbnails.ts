import fs from "node:fs";
import path from "node:path";

import type { CommandResult } from "./fileAssociations.ts";

/**
 * "Show picture thumbnails in File Explorer" (Windows only).
 *
 * Windows Explorer has no thumbnails for camera RAW, PSD and (without the
 * store extension) WebP. Mangoo ships a small COM thumbnail provider DLL
 * (`crates/explorer-thumbnails`) and registers it PER USER, in one
 * `reg import`, for exactly those extensions:
 *
 *   HKCU\Software\Classes\CLSID\{CLSID}\InprocServer32 = <dll>, Apartment
 *   HKCU\Software\Classes\.<ext>\ShellEx\{IThumbnailProvider} = {CLSID}
 *
 * JPEG / PNG / GIF / BMP / TIFF keep Windows' own provider. Unregistering
 * deletes only the keys this wrote (a ShellEx value is removed only when it
 * still names our CLSID, so another app's later handler is left alone).
 */

/** Must equal `CLSID_TEXT` in crates/explorer-thumbnails/src/lib.rs. */
export const EXPLORER_THUMBNAIL_CLSID = "{6F3C2B7A-41E8-4C5D-9A2E-8B1D7C0E5A93}";
/** Shell's IThumbnailProvider handler slot. */
export const THUMBNAIL_HANDLER_SLOT = "{e357fccd-a995-4576-b01f-234630154e96}";
export const EXPLORER_THUMBNAIL_DLL = "mangoo_explorer_thumbnails.dll";

/** Must equal `render::EXTENSIONS` in the provider crate (a test pins it). */
export const EXPLORER_THUMBNAIL_EXTENSIONS = [
  ".3fr", ".ari", ".arw", ".cr2", ".cr3", ".crw", ".dcr", ".dcs", ".dng", ".erf", ".fff", ".iiq",
  ".kdc", ".mef", ".mos", ".mrw", ".nef", ".nrw", ".orf", ".ori", ".pef", ".qtk", ".raf", ".raw",
  ".rw2", ".rwl", ".srw", ".x3f", ".webp", ".psd"
] as const;

export interface ExplorerThumbnailEnvironment {
  /** Absolute path of the provider DLL, or null when it is not shipped. */
  dllPath: string | null;
  tempDir: string;
  run(command: string, args: string[]): Promise<CommandResult>;
}

const CLASSES = "HKEY_CURRENT_USER\\Software\\Classes";

function regString(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

export function explorerThumbnailRegistrationScript(dllPath: string, extensions: readonly string[] = EXPLORER_THUMBNAIL_EXTENSIONS): string {
  const lines = [
    "Windows Registry Editor Version 5.00",
    "",
    `[${CLASSES}\\CLSID\\${EXPLORER_THUMBNAIL_CLSID}]`,
    `@=${regString("Mangoo 缩略图")}`,
    "",
    `[${CLASSES}\\CLSID\\${EXPLORER_THUMBNAIL_CLSID}\\InprocServer32]`,
    `@=${regString(dllPath)}`,
    `"ThreadingModel"="Apartment"`,
    ""
  ];
  for (const ext of extensions) {
    lines.push(`[${CLASSES}\\${ext}\\ShellEx\\${THUMBNAIL_HANDLER_SLOT}]`, `@=${regString(EXPLORER_THUMBNAIL_CLSID)}`, "");
  }
  return lines.join("\r\n");
}

/**
 * Removes the CLSID and every handler slot naming it. A slot that held another
 * app's per-user handler before we registered (`previous`) gets that value
 * back instead of being deleted, so turning the switch off is exact.
 */
export function explorerThumbnailUnregistrationScript(ours: readonly string[], previous: Readonly<Record<string, string>> = {}): string {
  const lines = [
    "Windows Registry Editor Version 5.00",
    "",
    `[-${CLASSES}\\CLSID\\${EXPLORER_THUMBNAIL_CLSID}]`,
    ""
  ];
  for (const ext of ours) {
    const restored = previous[ext];
    if (restored) {
      lines.push(`[${CLASSES}\\${ext}\\ShellEx\\${THUMBNAIL_HANDLER_SLOT}]`, `@=${regString(restored)}`, "");
    } else {
      lines.push(`[-${CLASSES}\\${ext}\\ShellEx\\${THUMBNAIL_HANDLER_SLOT}]`, "");
    }
  }
  return lines.join("\r\n");
}

async function importRegistry(env: ExplorerThumbnailEnvironment, script: string): Promise<void> {
  const file = path.join(env.tempDir, `mangoo-explorer-thumbnails-${process.pid}-${Date.now()}.reg`);
  fs.writeFileSync(file, Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(script, "utf16le")]));
  try {
    const result = await env.run("reg.exe", ["import", file]);
    if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `reg import exited with ${result.code}`);
  } finally {
    fs.rmSync(file, { force: true });
  }
}

/**
 * Current per-user thumbnail handler of each of our extensions, in ONE
 * `reg query` of the extensions' keys (missing keys are simply absent).
 * Returns lower-case extension -> handler CLSID text.
 */
export function parseHandlerQuery(stdout: string): Record<string, string> {
  const handlers: Record<string, string> = {};
  let current: string | null = null;
  const slot = THUMBNAIL_HANDLER_SLOT.toLowerCase();
  for (const line of stdout.split(/\r?\n/)) {
    const key = /\\Classes\\(\.[A-Za-z0-9]+)\\shellex\\(\{[^}]+\})\s*$/i.exec(line.trim());
    if (key) {
      current = key[2].toLowerCase() === slot ? key[1].toLowerCase() : null;
      continue;
    }
    const value = /^\s*\(Default\)\s+REG_SZ\s+(.*\S)\s*$/i.exec(line);
    if (current && value) {
      handlers[current] = value[1];
      current = null;
    } else if (/^HKEY_/i.test(line.trim())) {
      current = null;
    }
  }
  return handlers;
}

async function currentHandlers(env: ExplorerThumbnailEnvironment): Promise<Record<string, string>> {
  const result = await env.run("reg.exe", ["query", "HKCU\\Software\\Classes", "/s", "/f", THUMBNAIL_HANDLER_SLOT, "/k"]);
  if (result.code !== 0) return {};
  // Keys only; read the (few) matching slots of OUR extensions in one go each.
  const ours = new Set<string>(EXPLORER_THUMBNAIL_EXTENSIONS);
  const keys = [...new Set(result.stdout.split(/\r?\n/).map((line) => line.trim())
    .filter((line) => {
      const match = /\\Classes\\(\.[A-Za-z0-9]+)\\shellex\\/i.exec(line);
      return Boolean(match && ours.has(match[1].toLowerCase()));
    }))];
  const outputs = await Promise.all(keys.map((key) => env.run("reg.exe", ["query", key, "/ve"])));
  return parseHandlerQuery(outputs.map((output, at) => `${keys[at]}\r\n${output.code === 0 ? output.stdout : ""}`).join("\r\n"));
}

function namesUs(value: string | undefined): boolean {
  return Boolean(value && value.toUpperCase() === EXPLORER_THUMBNAIL_CLSID.toUpperCase());
}

/** Our extensions whose thumbnail slot currently names our CLSID. */
async function ownedExtensions(env: ExplorerThumbnailEnvironment): Promise<string[]> {
  const handlers = await currentHandlers(env);
  return EXPLORER_THUMBNAIL_EXTENSIONS.filter((ext) => namesUs(handlers[ext]));
}

/** Other apps' per-user handlers on our extensions (to restore later). */
export async function foreignHandlers(env: ExplorerThumbnailEnvironment): Promise<Record<string, string>> {
  const handlers = await currentHandlers(env);
  return Object.fromEntries(Object.entries(handlers).filter(([, value]) => !namesUs(value)));
}

export interface ExplorerThumbnailState {
  /** All our extensions point at our CLSID, and the CLSID names this DLL. */
  registered: boolean;
  /** The CLSID points at a DLL other than the shipped one (moved install). */
  stale: boolean;
}

export async function explorerThumbnailState(env: ExplorerThumbnailEnvironment): Promise<ExplorerThumbnailState> {
  const server = await env.run("reg.exe", ["query", `HKCU\\Software\\Classes\\CLSID\\${EXPLORER_THUMBNAIL_CLSID}\\InprocServer32`, "/ve"]);
  if (server.code !== 0) return { registered: false, stale: false };
  const pointsHere = Boolean(env.dllPath) && server.stdout.toLowerCase().includes(String(env.dllPath).toLowerCase());
  const owned = await ownedExtensions(env);
  return { registered: pointsHere && owned.length === EXPLORER_THUMBNAIL_EXTENSIONS.length, stale: !pointsHere };
}

/** Ask Explorer to refresh its handler cache (the DLL's own `Refresh` export). */
async function notifyShell(env: ExplorerThumbnailEnvironment, dllPath: string): Promise<void> {
  // rundll32 accepts the path unquoted even with spaces (verified).
  await env.run("rundll32.exe", [`${dllPath},Refresh`]).catch(() => undefined);
}

/**
 * Register; returns the other apps' handlers that were replaced, which the
 * caller persists and passes back to [`unregisterExplorerThumbnails`].
 */
export async function registerExplorerThumbnails(env: ExplorerThumbnailEnvironment): Promise<Record<string, string>> {
  if (!env.dllPath || !fs.existsSync(env.dllPath)) throw new Error("缺少缩略图组件，请重新安装 Mangoo");
  const replaced = await foreignHandlers(env);
  await importRegistry(env, explorerThumbnailRegistrationScript(env.dllPath));
  await notifyShell(env, env.dllPath);
  return replaced;
}

export async function unregisterExplorerThumbnails(env: ExplorerThumbnailEnvironment, previous: Readonly<Record<string, string>> = {}): Promise<void> {
  const owned = await ownedExtensions(env);
  await importRegistry(env, explorerThumbnailUnregistrationScript(owned, previous));
  if (env.dllPath && fs.existsSync(env.dllPath)) await notifyShell(env, env.dllPath);
}
