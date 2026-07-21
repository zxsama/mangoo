import { Bell } from "lucide-react";
import { StrictMode, useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/styles.css";
import { LiquidGlassButton, LiquidGlassLayer } from "../../src/design/liquid-glass";
import { NotificationPopover } from "../../src/features/notifications/NotificationPopover";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "../../src/features/notifications/notificationModel";
import "./notificationPopoverVisualFixture.css";

type FixtureState = "full" | "empty" | "degraded";

const fixtureParameters = new URLSearchParams(window.location.search);
const measurementMetrics = { anchorRectCalls: 0, popoverRectCalls: 0 };
const nativeGetBoundingClientRect = Element.prototype.getBoundingClientRect;

Element.prototype.getBoundingClientRect = function fixtureMeasurementRect() {
  if (this.matches('[data-titlebar-control="notifications"]')) {
    measurementMetrics.anchorRectCalls += 1;
  }
  if (this.matches('[data-compact-popover-root="notifications"]')) {
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
    ...overrides
  };
}

function fullModel(): CompactNotificationModel {
  return {
    active: [
      operation("重新索引 Artists"),
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
          completionPercent: 82
        }
      ],
      libraryCount: 1,
      cachedCount: 820,
      totalCandidateCount: 1000,
      missingCount: 120,
      staleCount: 24,
      failedCount: 4,
      pendingCount: 32,
      activeBulkTaskCount: 1,
      totalCacheBytes: 86 * 1024 * 1024,
      completionPercent: 82,
      active: true
    },
    results: [
      operation("删除旧目录", {
        operationId: "delete:archive",
        status: "succeeded",
        phase: "完成",
        summary: "已删除 128 个索引项",
        currentValue: 128,
        totalValue: 128,
        progress: { determinate: true, percent: 100, label: "128 / 128" },
        completedAt: Date.now() - 60_000
      }),
      operation("移动 Portraits", {
        operationId: "move:portraits",
        libraryId: "references",
        libraryName: "References",
        libraryLabel: "References",
        status: "failed",
        phase: "失败",
        errorDetail: "目标文件夹不可用",
        retryable: true,
        completedAt: Date.now() - 120_000
      }),
      operation("扫描 Archive", {
        operationId: "scan:archive",
        status: "cancelled",
        phase: "已取消",
        summary: "用户取消操作",
        completedAt: Date.now() - 180_000
      })
    ],
    indicator: "failure",
    degraded: false,
    healthDetail: null
  };
}

function modelFor(state: FixtureState): CompactNotificationModel {
  if (state === "full") return fullModel();
  return {
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
    requestedState === "empty" || requestedState === "degraded" ? requestedState : "full";
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
        <div className="notification-visual-fixture-stage">受控通知状态渲染器</div>
        {open && anchor ? (
          <NotificationPopover
            anchor={anchor}
            model={model}
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
    <NotificationPopoverVisualFixture />
  </StrictMode>
);
