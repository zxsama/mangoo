import fs from "node:fs";
import path from "node:path";

import { isAssociableExtension, mimeTypeForExtension } from "../shared/formats.ts";
import type { ImageViewerPlatform } from "../shared/ipc.ts";

/**
 * Registers Mangoo with the operating system as a handler ("Open with" and
 * default-app candidate) for the image formats the project decodes. Everything
 * is per-user and reversible:
 *
 * - Windows: HKCU ProgID + OpenWithProgids + Applications\<exe>\SupportedTypes
 *   + Capabilities/RegisteredApplications, written in ONE `reg import`.
 *   Windows 10/11 protect the actual default (UserChoice hash), so the final
 *   pick happens in Settings > Default apps, which we deep-link to.
 * - Linux: an XDG desktop entry under ~/.local/share/applications plus the
 *   [Default Applications] lines of ~/.config/mimeapps.list.
 * - macOS: document types are fixed in Info.plist at build time; runtime
 *   registration is not possible, so the status says so.
 *
 * Unregistering removes exactly what the recorded registration wrote.
 */

export interface CommandResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface AssociationEnvironment {
  platform: NodeJS.Platform;
  /** Executable the OS should launch (process.execPath, or $APPIMAGE). */
  executable: string;
  /** Arguments before the file (an unpackaged `electron main.js` launch). */
  launchArgs: string[];
  /** Short application key ("Mangoo"). */
  appKey: string;
  /** User-facing handler name ("Mangoo 看图"). */
  displayName: string;
  /** Windows ProgID ("Mangoo.Image"). */
  progId: string;
  iconPath: string | null;
  homeDir: string;
  tempDir: string;
  run(command: string, args: string[]): Promise<CommandResult>;
}

export const LINUX_DESKTOP_ID = "mangoo-image-viewer.desktop";

export function associationPlatform(platform: NodeJS.Platform): ImageViewerPlatform {
  return platform === "win32" || platform === "linux" || platform === "darwin" ? platform : "other";
}

export function associationSupported(platform: NodeJS.Platform): boolean {
  return platform === "win32" || platform === "linux";
}

function associableExtensions(extensions: readonly string[]): string[] {
  return [...new Set(extensions.map((ext) => ext.toLowerCase()).filter(isAssociableExtension))].sort();
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

function regString(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function windowsCommand(env: AssociationEnvironment): string {
  return [env.executable, ...env.launchArgs].map((part) => `"${part}"`).join(" ") + ' "%1"';
}

export function windowsExecutableName(executable: string): string {
  return path.win32.basename(executable);
}

export function windowsRegistrationScript(env: AssociationEnvironment, extensions: readonly string[]): string {
  const classes = "HKEY_CURRENT_USER\\Software\\Classes";
  const exeName = windowsExecutableName(env.executable);
  const capabilities = `HKEY_CURRENT_USER\\Software\\${env.appKey}\\ImageViewer\\Capabilities`;
  const command = windowsCommand(env);
  const icon = `"${env.executable}",0`;
  const lines = [
    "Windows Registry Editor Version 5.00",
    "",
    `[${classes}\\${env.progId}]`,
    `@=${regString(`${env.appKey} 图像`)}`,
    `"FriendlyTypeName"=${regString(`${env.appKey} 图像`)}`,
    "",
    `[${classes}\\${env.progId}\\DefaultIcon]`,
    `@=${regString(icon)}`,
    "",
    `[${classes}\\${env.progId}\\shell]`,
    `@="open"`,
    "",
    `[${classes}\\${env.progId}\\shell\\open]`,
    `"FriendlyAppName"=${regString(env.displayName)}`,
    "",
    `[${classes}\\${env.progId}\\shell\\open\\command]`,
    `@=${regString(command)}`,
    "",
    `[${classes}\\Applications\\${exeName}]`,
    `"FriendlyAppName"=${regString(env.displayName)}`,
    "",
    `[${classes}\\Applications\\${exeName}\\shell\\open\\command]`,
    `@=${regString(command)}`,
    "",
    `[${classes}\\Applications\\${exeName}\\SupportedTypes]`,
    ...extensions.map((ext) => `${regString(ext)}=""`),
    "",
    `[${capabilities}]`,
    `"ApplicationName"=${regString(env.displayName)}`,
    `"ApplicationDescription"=${regString("使用 Mangoo 查看图片")}`,
    `"ApplicationIcon"=${regString(icon)}`,
    "",
    `[${capabilities}\\FileAssociations]`,
    ...extensions.map((ext) => `${regString(ext)}=${regString(env.progId)}`),
    "",
    "[HKEY_CURRENT_USER\\Software\\RegisteredApplications]",
    `${regString(env.appKey)}=${regString(`Software\\${env.appKey}\\ImageViewer\\Capabilities`)}`,
    ""
  ];
  for (const ext of extensions) {
    lines.push(`[${classes}\\${ext}\\OpenWithProgids]`, `${regString(env.progId)}=""`, "");
  }
  return lines.join("\r\n");
}

export function windowsUnregistrationScript(
  env: AssociationEnvironment,
  registration: { executable: string; extensions: readonly string[] }
): string {
  const classes = "HKEY_CURRENT_USER\\Software\\Classes";
  const exeName = windowsExecutableName(registration.executable);
  const lines = [
    "Windows Registry Editor Version 5.00",
    "",
    `[-${classes}\\${env.progId}]`,
    "",
    `[-${classes}\\Applications\\${exeName}\\SupportedTypes]`,
    "",
    `[-${classes}\\Applications\\${exeName}\\shell\\open\\command]`,
    "",
    `[-HKEY_CURRENT_USER\\Software\\${env.appKey}\\ImageViewer]`,
    "",
    "[HKEY_CURRENT_USER\\Software\\RegisteredApplications]",
    `${regString(env.appKey)}=-`,
    ""
  ];
  for (const ext of registration.extensions) {
    lines.push(`[${classes}\\${ext}\\OpenWithProgids]`, `${regString(env.progId)}=-`, "");
  }
  return lines.join("\r\n");
}

async function importRegistry(env: AssociationEnvironment, script: string): Promise<void> {
  const file = path.join(env.tempDir, `mangoo-image-viewer-${process.pid}-${Date.now()}.reg`);
  // reg.exe expects UTF-16LE with a BOM for non-ASCII content.
  fs.writeFileSync(file, Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(script, "utf16le")]));
  try {
    const result = await env.run("reg.exe", ["import", file]);
    if (result.code !== 0) {
      throw new Error(result.stderr.trim() || result.stdout.trim() || `reg import exited with ${result.code}`);
    }
  } finally {
    fs.rmSync(file, { force: true });
  }
}

async function windowsRegistered(env: AssociationEnvironment): Promise<boolean> {
  const result = await env.run("reg.exe", [
    "query",
    `HKCU\\Software\\Classes\\${env.progId}\\shell\\open\\command`,
    "/ve"
  ]);
  return result.code === 0 && result.stdout.toLowerCase().includes(env.executable.toLowerCase());
}

// ---------------------------------------------------------------------------
// Linux (XDG)
// ---------------------------------------------------------------------------

function desktopExecArgument(value: string): string {
  return /[\s"'\\$`]/.test(value) ? `"${value.replace(/(["`$\\])/g, "\\$1")}"` : value;
}

export function linuxMimeTypes(extensions: readonly string[]): string[] {
  return [...new Set(extensions.map(mimeTypeForExtension).filter((value): value is string => Boolean(value)))].sort();
}

export function linuxDesktopEntry(env: AssociationEnvironment, extensions: readonly string[]): string {
  const exec = [env.executable, ...env.launchArgs].map(desktopExecArgument).join(" ");
  return [
    "[Desktop Entry]",
    "Type=Application",
    `Name=${env.displayName}`,
    "Comment=使用 Mangoo 查看图片",
    `Exec=${exec} %F`,
    ...(env.iconPath ? [`Icon=${env.iconPath}`] : []),
    "Terminal=false",
    "Categories=Graphics;Viewer;",
    `MimeType=${linuxMimeTypes(extensions).join(";")};`,
    ""
  ].join("\n");
}

/** Set (or, with `remove`, clear) our desktop id as default for `mimes`. */
export function updateMimeApps(text: string, desktopId: string, mimes: readonly string[], remove: boolean): string {
  const lines = text ? text.split(/\r?\n/) : [];
  const header = "[Default Applications]";
  let start = lines.findIndex((line) => line.trim() === header);
  if (start < 0) {
    if (remove) return text;
    if (lines.length && lines[lines.length - 1] !== "") lines.push("");
    lines.push(header);
    start = lines.length - 1;
  }
  let end = lines.findIndex((line, index) => index > start && /^\[.*\]\s*$/.test(line.trim()));
  if (end < 0) end = lines.length;
  const section = lines.slice(start + 1, end).filter((line) => {
    const [key, value] = line.split("=", 2);
    if (!mimes.includes(key?.trim() ?? "")) return true;
    // On remove, only drop entries that point at us; on add, replace them.
    return remove ? (value ?? "").trim() !== desktopId : false;
  });
  if (!remove) section.push(...mimes.map((mime) => `${mime}=${desktopId}`));
  const next = [...lines.slice(0, start + 1), ...section, ...lines.slice(end)];
  while (next.length > 1 && next[next.length - 1] === "" && next[next.length - 2] === "") next.pop();
  const joined = next.join("\n");
  return joined.endsWith("\n") ? joined : `${joined}\n`;
}

function linuxPaths(env: AssociationEnvironment) {
  const dataHome = process.env.XDG_DATA_HOME || path.join(env.homeDir, ".local", "share");
  const configHome = process.env.XDG_CONFIG_HOME || path.join(env.homeDir, ".config");
  return {
    applications: path.join(dataHome, "applications"),
    desktopFile: path.join(dataHome, "applications", LINUX_DESKTOP_ID),
    mimeApps: path.join(configHome, "mimeapps.list")
  };
}

function readText(file: string): string {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export async function registerAssociations(
  env: AssociationEnvironment,
  extensions: readonly string[]
): Promise<{ executable: string; extensions: string[] }> {
  const selected = associableExtensions(extensions);
  if (selected.length === 0) throw new Error("没有可注册的图片格式");
  if (env.platform === "win32") {
    await importRegistry(env, windowsRegistrationScript(env, selected));
  } else if (env.platform === "linux") {
    const paths = linuxPaths(env);
    fs.mkdirSync(paths.applications, { recursive: true });
    fs.writeFileSync(paths.desktopFile, linuxDesktopEntry(env, selected), { mode: 0o644 });
    fs.mkdirSync(path.dirname(paths.mimeApps), { recursive: true });
    fs.writeFileSync(paths.mimeApps, updateMimeApps(readText(paths.mimeApps), LINUX_DESKTOP_ID, linuxMimeTypes(selected), false));
    // Refresh the desktop database where the tool exists; harmless otherwise.
    await env.run("update-desktop-database", [paths.applications]).catch(() => undefined);
  } else {
    throw new Error("当前系统不支持在运行时注册默认看图应用");
  }
  return { executable: env.executable, extensions: selected };
}

export async function unregisterAssociations(
  env: AssociationEnvironment,
  registration: { executable: string; extensions: readonly string[] }
): Promise<void> {
  if (env.platform === "win32") {
    await importRegistry(env, windowsUnregistrationScript(env, registration));
  } else if (env.platform === "linux") {
    const paths = linuxPaths(env);
    fs.rmSync(paths.desktopFile, { force: true });
    const current = readText(paths.mimeApps);
    if (current) {
      fs.writeFileSync(paths.mimeApps, updateMimeApps(current, LINUX_DESKTOP_ID, linuxMimeTypes(registration.extensions), true));
    }
    await env.run("update-desktop-database", [paths.applications]).catch(() => undefined);
  }
}

export async function associationRegistered(env: AssociationEnvironment): Promise<boolean> {
  if (env.platform === "win32") return windowsRegistered(env).catch(() => false);
  if (env.platform === "linux") {
    const entry = readText(linuxPaths(env).desktopFile);
    return entry.includes(env.executable);
  }
  return false;
}

/** URL / command that opens the OS "pick a default app" surface. */
export function defaultAppsSettingsTarget(env: AssociationEnvironment): string | null {
  if (env.platform === "win32") {
    return `ms-settings:defaultapps?registeredAppUser=${encodeURIComponent(env.appKey)}`;
  }
  return null;
}
