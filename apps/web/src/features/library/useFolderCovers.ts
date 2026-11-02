import { useEffect, useMemo, useRef, useState } from "react";
import { subscribePluginChanges } from "../plugins/pluginQuerySync";
import type { FolderRecord, MediaRecord } from "@mangoo/core-client";
import { createCoreClient } from "../../core/client";

const FOLDER_COVER_FETCH_LIMIT = 96;
const FOLDER_COVER_CONCURRENT_FETCH_LIMIT = 8;
const FOLDER_COVER_MEDIA_LIMIT = 1;
const FOLDER_COVER_EMPTY_RETRY_MS = 3000;

export function useFolderCovers(
  folders: FolderRecord[],
  options: { disabled?: boolean; activeScan?: boolean } = {}
): Map<number, MediaRecord[]> {
  const client = useMemo(() => createCoreClient(), []);
  const disabled = options.disabled ?? false;
  const activeScan = options.activeScan ?? false;
  const [coversByFolderId, setCoversByFolderId] = useState<Map<number, MediaRecord[]>>(() => new Map());
  const inFlightFolderIds = useRef<Set<number>>(new Set());
  const inFlightControllersByFolderId = useRef<Map<number, AbortController>>(new Map());
  const mountedRef = useRef(true);
  const [requestVersion, setRequestVersion] = useState(0);

  useEffect(() => subscribePluginChanges(() => {
    abortAllControllers(inFlightControllersByFolderId.current);
    inFlightFolderIds.current.clear();
    setCoversByFolderId(new Map());
  }), []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      abortAllControllers(inFlightControllersByFolderId.current);
      inFlightFolderIds.current.clear();
    };
  }, []);

  useEffect(() => {
    if (disabled) {
      abortAllControllers(inFlightControllersByFolderId.current);
      inFlightFolderIds.current.clear();
      return;
    }

    const targetFolders = folders.slice(0, FOLDER_COVER_FETCH_LIMIT);
    const targetFolderIds = new Set(targetFolders.map((folder) => folder.id));
    for (const [folderId, controller] of inFlightControllersByFolderId.current) {
      if (!targetFolderIds.has(folderId)) {
        controller.abort();
        inFlightControllersByFolderId.current.delete(folderId);
        inFlightFolderIds.current.delete(folderId);
      }
    }

    const availableSlots = Math.max(
      0,
      FOLDER_COVER_CONCURRENT_FETCH_LIMIT - inFlightFolderIds.current.size
    );
    if (availableSlots === 0) {
      return;
    }

    const missingFolders = targetFolders
      .filter((folder) => !coversByFolderId.has(folder.id) && !inFlightFolderIds.current.has(folder.id))
      .slice(0, availableSlots);
    if (missingFolders.length === 0) {
      return;
    }

    for (const folder of missingFolders) {
      const controller = new AbortController();
      inFlightFolderIds.current.add(folder.id);
      inFlightControllersByFolderId.current.set(folder.id, controller);
      void (async () => {
        try {
          const page = await client.listMedia({
            folderId: folder.id,
            includeDescendants: true,
            limit: FOLDER_COVER_MEDIA_LIMIT,
            rootId: folder.rootId,
            sort: "name_asc"
          }, {
            requestPriority: "resource",
            signal: controller.signal
          });
          return page.items;
        } catch (cause) {
          if (isAbortError(cause)) {
            return null;
          }
          return [] as MediaRecord[];
        }
      })()
        .then((media) => {
          if (
            !mountedRef.current ||
            media === null ||
            inFlightControllersByFolderId.current.get(folder.id) !== controller
          ) {
            return;
          }
          setCoversByFolderId((current) => {
            if (current.has(folder.id)) {
              return current;
            }
            const next = new Map(current);
            next.set(folder.id, media);
            return next;
          });
        })
        .finally(() => {
          if (inFlightControllersByFolderId.current.get(folder.id) === controller) {
            inFlightControllersByFolderId.current.delete(folder.id);
            inFlightFolderIds.current.delete(folder.id);
            setRequestVersion((value) => value + 1);
          }
        });
    }
  }, [client, coversByFolderId, disabled, folders, requestVersion]);

  // A folder's cover is fetched once and cached — including an EMPTY result. But
  // with the structure-first scan the folder tree is committed before its media
  // is indexed, so a cover fetched early returns [] and, cached permanently,
  // never picks up the images indexed moments later (the folder preview then
  // "never refreshes"). While a scan is active, periodically drop cached-empty
  // entries so they re-fetch and reveal covers as media lands. Once the scan is
  // idle an empty result is final, so this stops and genuinely-empty folders are
  // not re-polled.
  useEffect(() => {
    if (disabled || !activeScan) {
      return;
    }
    const timer = window.setInterval(() => {
      setCoversByFolderId((current) => {
        let next: Map<number, MediaRecord[]> | null = null;
        for (const [folderId, media] of current) {
          if (media.length === 0) {
            if (!next) {
              next = new Map(current);
            }
            next.delete(folderId);
          }
        }
        return next ?? current;
      });
    }, FOLDER_COVER_EMPTY_RETRY_MS);
    return () => window.clearInterval(timer);
  }, [activeScan, disabled]);

  return coversByFolderId;
}

function abortAllControllers<K>(controllers: Map<K, AbortController>): void {
  for (const controller of controllers.values()) {
    controller.abort();
  }
  controllers.clear();
}

function isAbortError(cause: unknown): boolean {
  return cause instanceof Error && cause.name === "AbortError";
}
