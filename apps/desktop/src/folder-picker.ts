import type { DesktopFolderPickerRequest } from "@mangoo/notification-contract";

export interface FolderPickerDialogOptions {
  properties: ["openDirectory"];
  defaultPath?: string;
  title?: string;
}

export interface FolderPickerDialogResult {
  canceled: boolean;
  filePaths: string[];
}

export interface ShowFolderPickerDialog {
  (options: FolderPickerDialogOptions): Promise<FolderPickerDialogResult>;
  (owner: object, options: FolderPickerDialogOptions): Promise<FolderPickerDialogResult>;
}

const MODES = new Set<DesktopFolderPickerRequest["mode"]>([
  "enroll-existing",
  "materialize-remote",
  "relocate",
  "rebind-existing"
]);

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function isFolderPickerMode(value: string): value is DesktopFolderPickerRequest["mode"] {
  return MODES.has(value as DesktopFolderPickerRequest["mode"]);
}

export function parseFolderPickerRequest(value: unknown): DesktopFolderPickerRequest | null {
  if (!value || typeof value !== "object") return null;
  const input = value as Record<string, unknown>;
  const { libraryUuid, rootUuid, mode, suggestedPath, dialogTitle } = input;
  if (typeof libraryUuid !== "string" || !UUID_PATTERN.test(libraryUuid)) return null;
  if (rootUuid !== null && (typeof rootUuid !== "string" || !UUID_PATTERN.test(rootUuid))) return null;
  if (typeof mode !== "string" || !isFolderPickerMode(mode)) return null;
  if (suggestedPath !== null && typeof suggestedPath !== "string") return null;
  if (typeof dialogTitle !== "string" || !dialogTitle.trim()) return null;
  return {
    libraryUuid,
    rootUuid,
    mode,
    suggestedPath,
    dialogTitle
  };
}

export function folderPickerDialogOptions(request: DesktopFolderPickerRequest | null): FolderPickerDialogOptions {
  return {
    properties: ["openDirectory"],
    ...(request?.suggestedPath ? { defaultPath: request.suggestedPath } : {}),
    ...(request?.dialogTitle ? { title: request.dialogTitle } : {})
  };
}

export async function showFolderPicker(
  showOpenDialog: ShowFolderPickerDialog,
  owner: object | null,
  request: DesktopFolderPickerRequest | null
): Promise<string | null> {
  const options = folderPickerDialogOptions(request);
  const result = owner
    ? await showOpenDialog(owner, options)
    : await showOpenDialog(options);
  return result.canceled ? null : result.filePaths[0] ?? null;
}
