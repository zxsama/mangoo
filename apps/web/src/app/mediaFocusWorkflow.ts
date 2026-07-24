import type { MediaFocusCommand } from "@megle/notification-contract";

export interface MediaGridLocateRequest {
  requestId: string;
  mediaId: number;
}

export async function prepareMediaFocusNavigation(
  command: MediaFocusCommand,
  dependencies: {
    closePreview: () => void;
    locateMedia: (command: MediaFocusCommand) => Promise<boolean>;
  }
): Promise<MediaGridLocateRequest | null> {
  dependencies.closePreview();
  if (!(await dependencies.locateMedia(command))) return null;
  return { requestId: command.requestId, mediaId: command.fileId };
}

export function resolveMediaLocateVirtualIndex(input: {
  mediaIndex: number;
  columnCount: number;
  mediaRowStartIndex: number;
  mediaRowCount: number;
}): number | undefined {
  if (
    input.mediaIndex < 0 ||
    input.columnCount <= 0 ||
    input.mediaRowStartIndex < 0 ||
    input.mediaRowCount <= 0
  ) {
    return undefined;
  }
  const row = Math.floor(input.mediaIndex / input.columnCount);
  if (row >= input.mediaRowCount) return undefined;
  return input.mediaRowStartIndex + row;
}
