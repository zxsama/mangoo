import { Bell } from "lucide-react";
import { StrictMode, useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ThumbnailFailureDetail } from "@megle/notification-contract";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";
import { LiquidGlassButton, LiquidGlassLayer } from "../../src/design/liquid-glass";
import { OverlayProvider } from "../../src/design/overlays";
import { NotificationPopover } from "../../src/features/notifications/NotificationPopover";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "../../src/features/notifications/notificationModel";
import "./notificationPopoverVisualFixture.css";

type FixtureState = "full" | "scan" | "thumbnail" | "failure" | "empty" | "degraded";

const fixtureParameters = new URLSearchParams(window.location.search);
const measurementMetrics = { anchorRectCalls: 0, popoverRectCalls: 0 };
const nativeGetBoundingClientRect = Element.prototype.getBoundingClientRect;

function fixtureFailureDetails(
  libraryId: string,
  count: number,
  firstFileId: number
): ThumbnailFailureDetail[] {
  return Array.from({ length: count }, (_, index) => ({
    libraryId,
    fileId: firstFileId + index,
    folderId: 7,
    rootId: 3,
    relativePath: `${libraryId}\\failure-${index + 1}.jpg`,
    failureKind: "decode",
    error: "decode failed",
    updatedAt: Date.now() + index
  }));
}

Element.prototype.getBoundingClientRect = function fixtureMeasurementRect() {
  if (this.matches('[data-titlebar-control="notifications"]')) {
    measurementMetrics.anchorRectCalls += 1;
  }
  if (this.matches("#notifications-root")) {
    measurementMetrics.popoverRectCalls += 1;
  }
  return nativeGetBoundingClientRect.call(this);
};

type FixtureAnchorLocation = "center" | "right";

interface NotificationFixtureController {
  churnDetachedPointerStyles(count: number): void;
  churnPointerStyles(count: number): void;
  getMetrics(): {
    anchorId: string | null;
    anchorRectCalls: number;
    detachedAnchorCount: number;
    location: FixtureAnchorLocation;
    popoverRectCalls: number;
  };
  relocate(location: FixtureAnchorLocation): void;
  remount(): void;
  resetMeasurements(): void;
}

type FixtureWindow = Window & {
  notificationPopoverFixture?: NotificationFixtureController;
};

function operation(
  operationId: string,
  overrides: Partial<NotificationOperationViewModel> = {}
): NotificationOperationViewModel {
  return {
    operationId,
    revision: 1,
    kind: "root_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status: "running",
    phase: "扫描媒体",
    title: operationId,
    summary: null,
    taskId: 1,
    rootId: 1,
    folderId: null,
    fileId: null,
    currentValue: 42,
    totalValue: 100,
    unit: "items",
    errorCode: null,
    errorDetail: null,
    cancellable: true,
    retryable: false,
    attempt: 1,
    occurrenceCount: 1,
    startedAt: Date.now() - 12_000,
    updatedAt: Date.now(),
    completedAt: null,
    libraryId: "artists",
    libraryName: "Artists",
    libraryLabel: "Artists",
    progress: { determinate: true, percent: 42, label: "42 / 100" },
    sourceOperations: [],
    scanPaths: [],
    ...overrides
  };
}

function fullModel(): CompactNotificationModel {
  return {
    activeLibraryId: "artists",
    active: [
      operation("重新索引 Artists", {
        scanPaths: [
          {
            key: "artists:portraits",
            libraryId: "artists",
            operationId: "scan:portraits",
            path: "Artists\\Portraits"
          },
          {
            key: "artists:editorial",
            libraryId: "artists",
            operationId: "scan:editorial",
            path: "Artists\\Editorial"
          },
          {
            key: "artists:archive",
            libraryId: "artists",
            operationId: "scan:archive",
            path: "Artists\\Archive"
          }
        ]
      }),
      operation("分析 References", {
        operationId: "scan:references",
        libraryId: "references",
        libraryName: "References",
        libraryLabel: "References",
        phase: "准备文件",
        currentValue: 0,
        totalValue: null,
        progress: { determinate: false, percent: null, label: "准备文件" }
      })
    ],
    thumbnail: {
      summaries: [
        {
          libraryId: "artists",
          libraryName: "Artists",
          observedAt: Date.now(),
          cachedCount: 820,
          totalCandidateCount: 1000,
          missingCount: 120,
          staleCount: 24,
          failedCount: 4,
          pendingCount: 32,
          activeBulkTaskCount: 1,
          totalCacheBytes: 86 * 1024 * 1024,
          completionPercent: 82,
          failureDetails: fixtureFailureDetails("artists", 4, 42),
          failureDetailsTruncated: false,
          failureDetailsUnavailable: false
        },
        {
          libraryId: "references",
          libraryName: "References",
          observedAt: Date.now(),
          cachedCount: 500,
          totalCandidateCount: 800,
          missingCount: 220,
          staleCount: 48,
          failedCount: 9,
          pendingCount: 18,
          activeBulkTaskCount: 2,
          totalCacheBytes: 54 * 1024 * 1024,
          completionPercent: 63,
          failureDetails: fixtureFailureDetails("references", 9, 84),
          failureDetailsTruncated: false,
          failureDetailsUnavailable: false
        }
      ],
      libraryCount: 2,
      cachedCount: 1320,
      totalCandidateCount: 1800,
      missingCount: 340,
      staleCount: 72,
      failedCount: 13,
      pendingCount: 50,
      activeBulkTaskCount: 3,
      totalCacheBytes: 140 * 1024 * 1024,
      completionPercent: 73,
      active: true
    },
    results: [],
    indicator: "failure",
    degraded: false,
    healthDetail: null
  };
}

function modelFor(state: FixtureState): CompactNotificationModel {
  const full = fullModel();
  if (state === "full") return full;
  if (state === "scan") {
    return {
      ...full,
      thumbnail: {
        summaries: [],
        libraryCount: 0,
        cachedCount: 0,
        totalCandidateCount: 0,
        missingCount: 0,
        staleCount: 0,
        failedCount: 0,
        pendingCount: 0,
        activeBulkTaskCount: 0,
        totalCacheBytes: null,
        completionPercent: null,
        active: false
      },
      indicator: "active"
    };
  }
  if (state === "thumbnail") {
    return {
      ...full,
      active: [],
      indicator: "active"
    };
  }
  const idle: CompactNotificationModel = {
    activeLibraryId: null,
    active: [],
    thumbnail: {
      summaries: [],
      libraryCount: 0,
      cachedCount: 0,
      totalCandidateCount: 0,
      missingCount: 0,
      staleCount: 0,
      failedCount: 0,
      pendingCount: 0,
      activeBulkTaskCount: 0,
      totalCacheBytes: null,
      completionPercent: null,
      active: false
    },
    results: [],
    indicator: "idle",
    degraded: state === "degraded",
    healthDetail: state === "degraded" ? "测试同步连接已暂停" : null
  };
  if (state !== "failure") return idle;
  return {
    ...idle,
    activeLibraryId: "artists",
    results: [
      operation("扫描 Artists", {
        status: "failed",
        phase: "扫描失败",
        summary: "fixture terminal detail must not render",
        cancellable: false,
        retryable: true,
        completedAt: Date.now()
      })
    ],
    indicator: "failure"
  };
}

function NotificationPopoverVisualFixture() {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [anchorLocation, setAnchorLocation] = useState<FixtureAnchorLocation>("right");
  const [anchorVersion, setAnchorVersion] = useState(0);
  const anchorNodeRef = useRef<HTMLButtonElement | null>(null);
  const detachedAnchorsRef = useRef<HTMLButtonElement[]>([]);
  const [open, setOpen] = useState(true);
  const requestedState = fixtureParameters.get("state");
  const state: FixtureState =
    requestedState === "scan" ||
    requestedState === "thumbnail" ||
    requestedState === "failure" ||
    requestedState === "empty" ||
    requestedState === "degraded"
      ? requestedState
      : "full";
  const model = modelFor(state);
  const anchorId = `notification-anchor-${anchorLocation}-${anchorVersion}`;
  const bindAnchor = useCallback((node: HTMLButtonElement | null) => {
    const previous = anchorNodeRef.current;
    if (previous && previous !== node && !detachedAnchorsRef.current.includes(previous)) {
      detachedAnchorsRef.current.push(previous);
    }
    anchorNodeRef.current = node;
    setAnchor(node);
  }, []);
  const notificationButton = (
    <LiquidGlassButton
      active={open}
      aria-label="通知"
      className="titlebar-icon-button notification-toggle no-drag"
      data-compact-popover-trigger="notifications"
      data-fixture-anchor-id={anchorId}
      data-titlebar-control="notifications"
      key={anchorId}
      onClick={() => setOpen((current) => !current)}
      ref={bindAnchor}
      tone="control"
    >
      <Bell aria-hidden="true" size={15} />
      {model.indicator === "idle" ? null : (
        <span className={`notification-status-dot ${model.indicator}`} />
      )}
    </LiquidGlassButton>
  );

  (window as FixtureWindow).notificationPopoverFixture = {
    churnDetachedPointerStyles(count) {
      const detached = detachedAnchorsRef.current.at(-1);
      for (let index = 0; detached && index < count; index += 1) {
        detached.style.setProperty("--glass-pointer-x", `${index % 100}%`);
      }
    },
    churnPointerStyles(count) {
      const current = anchorNodeRef.current;
      for (let index = 0; current && index < count; index += 1) {
        current.style.setProperty("--glass-pointer-x", `${index % 100}%`);
        current.style.setProperty("--glass-pointer-y", `${(index * 3) % 100}%`);
      }
    },
    getMetrics() {
      return {
        anchorId: anchorNodeRef.current?.dataset.fixtureAnchorId ?? null,
        anchorRectCalls: measurementMetrics.anchorRectCalls,
        detachedAnchorCount: detachedAnchorsRef.current.length,
        location: anchorLocation,
        popoverRectCalls: measurementMetrics.popoverRectCalls
      };
    },
    relocate(location) {
      setAnchorLocation(location);
      setAnchorVersion((current) => current + 1);
    },
    remount() {
      setAnchorVersion((current) => current + 1);
    },
    resetMeasurements() {
      measurementMetrics.anchorRectCalls = 0;
      measurementMetrics.popoverRectCalls = 0;
    }
  };

  return (
    <LiquidGlassLayer>
      <main className="notification-visual-fixture" data-notification-fixture={state}>
        <div className="notification-visual-fixture-titlebars">
          <header
            className="notification-visual-fixture-titlebar shell-titlebar shell-titlebar-center"
            data-fixture-titlebar="center"
          >
            <div className="shell-right-actions no-drag">
              {anchorLocation === "center" ? notificationButton : null}
            </div>
          </header>
          <header
            className="notification-visual-fixture-titlebar shell-titlebar shell-titlebar-right"
            data-fixture-titlebar="right"
          >
          <div className="shell-right-actions no-drag">
              {anchorLocation === "right" ? notificationButton : null}
          </div>
          </header>
        </div>
        <div className="notification-visual-fixture-stage">
          受控通知状态：{state}
        </div>
        {open && anchor ? (
          <NotificationPopover
            anchor={anchor}
            model={model}
            onClose={() => setOpen(false)}
            onOpenCenter={() => setOpen(false)}
          />
        ) : null}
      </main>
    </LiquidGlassLayer>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("notification fixture root is missing");
createRoot(root).render(
  <StrictMode>
    <OverlayProvider>
      <NotificationPopoverVisualFixture />
    </OverlayProvider>
  </StrictMode>
);
