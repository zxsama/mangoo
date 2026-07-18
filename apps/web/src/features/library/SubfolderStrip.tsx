import { Check, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Folder } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";
import type { FolderRecord, MediaRecord } from "@megle/core-client";
import {
  mediaFileContentSignature,
  readCachedThumbnailObjectUrl,
  rememberThumbnailObjectUrl,
  requestOriginalPreviewBlob,
  requestThumbnailBlob,
  requestThumbnailState,
  thumbnailObjectUrlCacheKey
} from "../../core/mediaResources";
import type { SubfolderSiblingPosition } from "./subfolderHierarchy";
import { MediaLoadingRing } from "../loading/MediaLoadingRing";

interface SubfolderStripProps {
  collapsed: boolean;
  folderCount: number;
  loading: boolean;
  showChildContents: boolean;
  onToggleCollapsed: () => void;
  onToggleShowChildContents: () => void;
}

interface SubfolderCardProps {
  coverLoaded: boolean;
  coverMedia: MediaRecord[];
  depth?: number;
  expandable?: boolean;
  expanded?: boolean;
  childStatus?: "empty" | "has-children" | "loading" | "unknown";
  hasExpandedChildren?: boolean;
  folder: FolderRecord;
  inheritedGroupPosition?: SubfolderSiblingPosition | null;
  loadingChildren?: boolean;
  nestedGroupPosition?: SubfolderSiblingPosition | null;
  selected: boolean;
  onFolderContextMenu?: (event: {
    folder: FolderRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
  onOpenFolder: (folder: FolderRecord) => void;
  onSelectFolder: (folder: FolderRecord) => void;
  onToggleExpanded?: (folder: FolderRecord) => void;
}

export function SubfolderStrip({
  collapsed,
  folderCount,
  loading,
  showChildContents,
  onToggleCollapsed,
  onToggleShowChildContents
}: SubfolderStripProps) {
  const title = useMemo(() => {
    if (loading && folderCount === 0) {
      return "子文件夹";
    }
    return `子文件夹 (${folderCount})`;
  }, [folderCount, loading]);

  return (
    <section
      className={`subfolder-strip${collapsed ? " is-collapsed" : ""}`}
      aria-label="Child folders"
    >
      <header className="subfolder-strip-header">
        <button
          aria-label={collapsed ? "Expand child folders" : "Collapse child folders"}
          className="subfolder-strip-header-button subfolder-strip-heading-button"
          data-pointer-glow="off"
          onClick={onToggleCollapsed}
          type="button"
        >
          <span className="subfolder-strip-title">{title}</span>
          <span className="subfolder-strip-heading-icon" aria-hidden="true">
            {collapsed ? <ChevronDown size={15} /> : <ChevronUp size={15} />}
          </span>
        </button>

        <label className="subfolder-strip-header-button library-browser-content-toggle">
          <input
            checked={showChildContents}
            onChange={() => onToggleShowChildContents()}
            type="checkbox"
          />
          <span className="library-browser-content-toggle-indicator" aria-hidden="true">
            <Check size={12} />
          </span>
          <span className="library-browser-content-toggle-label">显示子文件夹内容</span>
        </label>
      </header>
    </section>
  );
}

export function SubfolderCard({
  coverLoaded,
  coverMedia,
  depth = 0,
  expandable = false,
  expanded = false,
  childStatus = "unknown",
  hasExpandedChildren = false,
  folder,
  inheritedGroupPosition = null,
  loadingChildren = false,
  nestedGroupPosition = null,
  onFolderContextMenu,
  onOpenFolder,
  onSelectFolder,
  onToggleExpanded,
  selected
}: SubfolderCardProps) {
  const isNested = depth > 0;
  const nestedCoverScale = isNested ? Math.max(0.6, Math.pow(0.9, depth)) : 1;
  const parentCoverScale = isNested ? Math.max(0.6, Math.pow(0.9, Math.max(0, depth - 1))) : 1;
  const style = {
    "--subfolder-depth": String(depth),
    "--subfolder-depth-indent": "0px",
    "--subfolder-nested-cover-scale": nestedCoverScale.toFixed(4),
    "--subfolder-parent-cover-scale": parentCoverScale.toFixed(4)
  } as CSSProperties;

  return (
    <div
      className={selected ? "subfolder-card selected" : "subfolder-card"}
      data-cover-count={coverMedia.length}
      data-child-status={childStatus}
      data-depth={depth}
      data-expanded-children={hasExpandedChildren ? "true" : undefined}
      data-inherited-position={isNested ? inheritedGroupPosition ?? "single" : undefined}
      data-nested={isNested ? "true" : undefined}
      data-nested-layer={depth > 1 ? "true" : undefined}
      data-nested-position={isNested ? nestedGroupPosition ?? "single" : undefined}
      style={style}
    >
      <button
        aria-label={`Select ${folder.name}; double-click to open folder`}
        aria-pressed={selected}
        className="subfolder-card-main"
        data-interactive-pointer-target-selector=".subfolder-card-pointer-surface"
        onClick={() => onSelectFolder(folder)}
        onDoubleClick={() => onOpenFolder(folder)}
        onContextMenu={(event) => {
          if (!onFolderContextMenu) return;
          event.preventDefault();
          onFolderContextMenu({
            folder,
            x: event.clientX,
            y: event.clientY,
            shiftKey: event.shiftKey
          });
        }}
        type="button"
      >
        <span className="subfolder-card-cover" aria-hidden="true">
          <FolderCoverPreview coverLoaded={coverLoaded} coverMedia={coverMedia} />
          <span className="subfolder-card-pointer-surface" />
          <span className="library-thumbnail-interaction-ring" />
        </span>
        <span className="subfolder-card-copy">
          <span className="subfolder-card-name" title={folder.name}>
            {folder.name}
          </span>
        </span>
      </button>
      {expandable ? (
        <button
          aria-label={expanded ? `Collapse ${folder.name}` : `Expand ${folder.name}`}
          aria-pressed={expanded}
          className="subfolder-card-expand"
          disabled={loadingChildren}
          onClick={(event) => {
            event.stopPropagation();
            onToggleExpanded?.(folder);
          }}
          title={expanded ? "Collapse folder" : "Expand folder"}
          type="button"
        >
          {loadingChildren ? (
            <MediaLoadingRing className="media-loading-ring subfolder-card-expand-spinner" size={13} />
          ) : expanded ? (
            <ChevronLeft size={14} />
          ) : (
            <ChevronRight size={14} />
          )}
        </button>
      ) : null}
    </div>
  );
}

function FolderCoverPreview({
  coverLoaded,
  coverMedia
}: {
  coverLoaded: boolean;
  coverMedia: MediaRecord[];
}) {
  const coverMediaItem = coverMedia[0] ?? null;
  const cacheKey = coverMediaItem
    ? thumbnailObjectUrlCacheKey(
        coverMediaItem.id,
        `folder-cover:${mediaFileContentSignature(coverMediaItem)}`,
        null
      )
    : null;
  const [src, setSrc] = useState<string | null>(() =>
    cacheKey ? readCachedThumbnailObjectUrl(cacheKey) : null
  );
  const [failed, setFailed] = useState(false);
  const [retryTick, setRetryTick] = useState(0);
  const objectUrlRef = useRef<string | null>(src);
  const cacheOwnsObjectUrlRef = useRef(src !== null);
  // Fade the cover in only when it was NOT already cached at mount, matching the
  // media-grid tiles: a fresh cover fades in (0.33s), a cover scrolled back into
  // view with its object URL still cached renders instantly with no re-flash.
  const shouldFadeInRef = useRef(src === null);

  useEffect(() => {
    if (!coverMediaItem || !cacheKey) {
      setSrc(null);
      setFailed(false);
      objectUrlRef.current = null;
      cacheOwnsObjectUrlRef.current = false;
      return undefined;
    }

    const cachedObjectUrl = readCachedThumbnailObjectUrl(cacheKey);
    if (cachedObjectUrl) {
      objectUrlRef.current = cachedObjectUrl;
      cacheOwnsObjectUrlRef.current = true;
      setSrc(cachedObjectUrl);
      setFailed(false);
      return undefined;
    }

    let revoked = false;
    let objectUrl: string | null = null;
    const controller = new AbortController();
    setFailed(false);

    requestThumbnailState(coverMediaItem, "visible", { signal: controller.signal })
      .then((thumbnail) => {
        if (thumbnail.state === "ready" && thumbnail.updatedAt !== null) {
          return requestThumbnailBlob(coverMediaItem.id, thumbnail.updatedAt, {
            requestPriority: "resource",
            resourcePriority: "visible",
            signal: controller.signal
          });
        }
        return requestOriginalPreviewBlob(coverMediaItem, {
          requestPriority: "resource",
          resourcePriority: "visible",
          signal: controller.signal
        });
      })
      .catch((cause) => {
        if (isAbortError(cause)) {
          throw cause;
        }
        return requestOriginalPreviewBlob(coverMediaItem, {
          requestPriority: "resource",
          resourcePriority: "visible",
          signal: controller.signal
        });
      })
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        if (revoked) {
          URL.revokeObjectURL(objectUrl);
          return;
        }
        objectUrlRef.current = objectUrl;
        cacheOwnsObjectUrlRef.current = rememberThumbnailObjectUrl(
          cacheKey,
          objectUrl,
          blob.size
        );
        setSrc(objectUrl);
        setFailed(false);
      })
      .catch((cause) => {
        if (objectUrl && objectUrlRef.current !== objectUrl) {
          URL.revokeObjectURL(objectUrl);
          objectUrl = null;
        }
        if (!revoked && !isAbortError(cause)) {
          setFailed(true);
          window.setTimeout(() => {
            setRetryTick((value) => value + 1);
          }, 1000);
        }
      });

    return () => {
      revoked = true;
      controller.abort();
      if (objectUrl && objectUrlRef.current !== objectUrl) {
        URL.revokeObjectURL(objectUrl);
      }
      if (objectUrl && objectUrlRef.current === objectUrl && !cacheOwnsObjectUrlRef.current) {
        URL.revokeObjectURL(objectUrl);
        objectUrlRef.current = null;
      }
    };
    // Keyed on `cacheKey` (folder-cover content identity), NOT the `coverMediaItem`
    // object: useFolderCovers hands back a fresh array/object on each viewport
    // re-prioritization, so depending on the object identity re-ran this effect
    // and re-fetched the cover on every scroll — the cover flashed its background
    // and reloaded. cacheKey stays constant while the cover's content is the same.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cacheKey, retryTick]);

  if (!src || failed) {
    const coverStatus = failed ? "failed" : coverMediaItem ? "loading" : coverLoaded ? "empty" : "loading";
    return (
      <span
        className="subfolder-card-cover-fallback"
        data-cover-status={coverStatus}
      >
        <Folder size={22} />
      </span>
    );
  }

  return (
    <span className="subfolder-card-cover-image-frame" data-cover-status="ready">
      <img
        alt=""
        className={
          shouldFadeInRef.current
            ? "subfolder-card-cover-image subfolder-card-cover-image--fade"
            : "subfolder-card-cover-image"
        }
        loading="lazy"
        src={src}
      />
    </span>
  );
}

function isAbortError(cause: unknown): boolean {
  return cause instanceof Error && cause.name === "AbortError";
}
