import path from "node:path";

/**
 * File paths the operating system passed on the command line. Electron and
 * Chromium switches (`--foo`, `-foo`) are ignored, as is the app script in an
 * unpackaged `electron main.js` launch. Relative paths resolve against the
 * launching process' working directory (second-instance supplies it).
 */
export function extractOpenPaths(
  argv: readonly string[],
  options: { defaultApp: boolean; cwd: string; platform?: NodeJS.Platform }
): string[] {
  const resolver = (options.platform ?? process.platform) === "win32" ? path.win32 : path.posix;
  const skip = options.defaultApp ? 2 : 1;
  const result: string[] = [];
  for (const raw of argv.slice(skip)) {
    if (typeof raw !== "string") continue;
    const value = raw.trim();
    if (!value || value.startsWith("-") || value.includes("\0") || value.length > 4096) continue;
    // Protocol URLs and Electron's own `.` app path are not files to open.
    if (/^[a-z][a-z0-9+.-]*:\/\//i.test(value) || value === ".") continue;
    const absolute = resolver.isAbsolute(value) ? resolver.normalize(value) : resolver.resolve(options.cwd, value);
    if (!result.includes(absolute)) result.push(absolute);
  }
  return result;
}
