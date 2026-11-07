import path from "node:path";

import { extensionOf } from "../shared/formats.ts";

/**
 * Which local paths the viewer renderer may act on (trash, rename, reveal,
 * open externally). Only images directly inside a folder that the USER opened
 * in this viewer session — through the OS, the open dialog, drag and drop or
 * the Library — are admitted. A compromised renderer therefore cannot ask the
 * Desktop host to delete arbitrary files elsewhere on disk.
 */
export class ViewerAccessPolicy {
  private readonly folders = new Map<string, string>();

  private readonly platform: NodeJS.Platform;

  constructor(platform: NodeJS.Platform = process.platform) {
    this.platform = platform;
  }

  private key(folder: string): string {
    const normalized = this.resolver().normalize(folder).replace(/[\\/]+$/, "");
    return this.platform === "win32" ? normalized.toLowerCase() : normalized;
  }

  private resolver(): typeof path.win32 {
    return this.platform === "win32" ? path.win32 : path.posix;
  }

  admitFolder(folder: string): void {
    if (!this.resolver().isAbsolute(folder)) return;
    this.folders.set(this.key(folder), folder);
  }

  admitFile(file: string): void {
    if (!this.resolver().isAbsolute(file)) return;
    this.admitFolder(this.resolver().dirname(file));
  }

  /** An absolute, normalized image path whose folder was admitted. */
  allows(file: string, supportedExtensions: ReadonlySet<string>): boolean {
    const resolver = this.resolver();
    if (typeof file !== "string" || !resolver.isAbsolute(file) || file.includes("\0")) return false;
    if (file.split(/[\\/]/).some((part) => part === ".." || part === ".")) return false;
    if (!supportedExtensions.has(extensionOf(file))) return false;
    return this.folders.has(this.key(resolver.dirname(file)));
  }

  clear(): void {
    this.folders.clear();
  }
}
