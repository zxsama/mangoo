import path from "node:path";
import type { LibraryCreationPaths, ResolveLibraryCreationPathsRequest } from "@mangoo/notification-contract";

/** Read-only path arithmetic. Core owns inspection, directory creation and admission. */
export function resolveLibraryCreationPaths(
  documentsPath: string,
  input: ResolveLibraryCreationPathsRequest,
  platform: NodeJS.Platform = process.platform
): LibraryCreationPaths {
  if (!input || typeof input.name !== "string" ||
      (input.databasePath !== undefined && typeof input.databasePath !== "string")) {
    throw new Error("Invalid Library path request");
  }
  const paths = platform === "win32" ? path.win32 : path.posix;
  if (input.databasePath !== undefined) {
    const driveAbsolute = platform !== "win32" || /^(?:[a-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$))/i.test(input.databasePath);
    if (!paths.isAbsolute(input.databasePath) || !driveAbsolute || input.databasePath.includes("\0")) {
      throw new Error("请输入数据库目录的完整路径。");
    }
    return { databasePath: input.databasePath, defaultContentPath: paths.join(paths.dirname(input.databasePath), "Files") };
  }
  const name = input.name;
  if (!name || name === "." || name === ".." || /[<>:"/\\|?*\u0000-\u001f]/.test(name) ||
      /[. ]$/.test(name) || /^(con|prn|aux|nul|conin\$|conout\$|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/i.test(name)) {
    throw new Error("名称不能用于自动目录，请修改名称。");
  }
  if (!paths.isAbsolute(documentsPath)) throw new Error("无法确定文档目录，请选择自定义数据库位置。");
  const base = paths.join(documentsPath, "Mangoo", "Libraries", name);
  return { databasePath: paths.join(base, "Database"), defaultContentPath: paths.join(base, "Files") };
}
