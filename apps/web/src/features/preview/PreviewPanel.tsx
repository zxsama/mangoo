import { FolderGlyph } from "../library/FolderGlyph";
import { type ReactNode } from "react";
import type { FolderRecord, MediaRecord, ThumbnailResponse } from "@mangoo/core-client";
import { MediaLoadingRing } from "../loading/MediaLoadingRing";
import { MediaPreview } from "./MediaPreview";
import { FileFormatBadge } from "../media-grid/FileFormatArtwork";
import { isThumbnailUnavailable } from "../../core/mediaResources";

interface PreviewPanelProps {
  selectedFolder: FolderRecord | null;
  selectedFolderCoverMedia: MediaRecord[];
  selectedFolderCoverThumbnail?: ThumbnailResponse;
  selectedMedia: MediaRecord | null;
  thumbnail?: ThumbnailResponse;
  showPreviewImage?: boolean;
  // Suppress the "No selection" placeholder (e.g. during a library switch, when
  // the transient empty state should read as blank, not as a real empty panel).
  suppressEmptyState?: boolean;
  children?: ReactNode;
}

export function PreviewPanel({
  selectedFolder,
  selectedFolderCoverMedia,
  selectedFolderCoverThumbnail,
  selectedMedia,
  showPreviewImage = true,
  suppressEmptyState = false,
  thumbnail,
  children
}: PreviewPanelProps) {
  const folderCoverMedia = selectedFolderCoverMedia[0] ?? null;

  return (
    <aside
      className="inspector-panel preview-panel"
      aria-label="Preview"
    >
      {selectedMedia ? (
        <>
          {showPreviewImage ? (
            <div className="preview-stage">
              <MediaPreview
                media={selectedMedia}
                onMediaReady={undefined}
                source="thumbnail"
                thumbnail={thumbnail}
                preserveNaturalFrame={false}
                preferOriginalWhilePending
              />
              <FileFormatBadge file={selectedMedia} />
            </div>
          ) : null}
          <dl className="metadata-list">
            <dt>Name</dt>
            <dd>{selectedMedia.name}</dd>
            <dt>Kind</dt>
            <dd>{selectedMedia.kind ?? "unknown"}</dd>
            <dt>Size</dt>
            <dd>{formatBytes(selectedMedia.size)}</dd>
            <dt>Dimensions</dt>
            <dd>{formatDimensions(selectedMedia)}</dd>
            <dt>Thumbnail</dt>
            <dd>{thumbnailStatus(thumbnail?.state ?? (isThumbnailUnavailable(selectedMedia) && selectedMedia.thumbnailState !== "ready" ? "unavailable" : selectedMedia.thumbnailState))}</dd>
          </dl>
          {children}
        </>
      ) : selectedFolder ? (
        <>
          <div className="preview-stage">
            {folderCoverMedia ? (
              <MediaPreview
                media={folderCoverMedia}
                onMediaReady={undefined}
                source="thumbnail"
                thumbnail={selectedFolderCoverThumbnail}
                preserveNaturalFrame={false}
                preferOriginalWhilePending
              />
            ) : (
              <div className="preview-folder-cover-empty" aria-hidden="true">
                <FolderGlyph icon={selectedFolder.icon} color={selectedFolder.color} size={34} />
              </div>
            )}
          </div>
          <dl className="metadata-list">
            <dt>Name</dt>
            <dd>{selectedFolder.name}</dd>
            <dt>Kind</dt>
            <dd>folder</dd>
            <dt>Root</dt>
            <dd>{selectedFolder.rootId}</dd>
            <dt>Parent</dt>
            <dd>{selectedFolder.parentId ?? "root"}</dd>
          </dl>
        </>
      ) : (
        suppressEmptyState ? null : (
          <div className="empty-panel">No selection</div>
        )
      )}
    </aside>
  );
}

function thumbnailStatus(state: ThumbnailResponse["state"] | string | null | undefined): ReactNode {
  if (state === "ready") {
    return "Ready";
  }
  if (state === "failed") {
    return "Failed";
  }
  if (state === "skipped_small") {
    return "Skipped";
  }
  if (state === "unavailable") return "File icon";
  return (
    <span aria-label="Thumbnail refreshing" className="metadata-inline-spinner" role="status">
      <MediaLoadingRing size={16} />
    </span>
  );
}

function formatDimensions(media: MediaRecord): string {
  if (media.width && media.height) {
    return `${media.width} x ${media.height}`;
  }
  return "unknown";
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}
