import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { NotificationOperationViewModel } from "../src/features/notifications/notificationModel.ts";
import { resolveNotificationPopoverPosition } from "../src/features/notifications/notificationPopoverPosition.ts";
import {
  observeNotificationPopoverMeasurements,
  type NotificationPopoverMeasurementEnvironment
} from "../src/features/notifications/notificationPopoverMeasurement.ts";
import {
  activeOperationPresentation,
  resultOperationPresentation
} from "../src/features/notifications/notificationPopoverPresentation.ts";

function operation(
  overrides: Partial<NotificationOperationViewModel> = {}
): NotificationOperationViewModel {
  return {
    operationId: "scan:artists",
    revision: 2,
    kind: "root_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status: "running",
    phase: "索引文件",
    title: "扫描 Artists",
    summary: "扫描完成",
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
    startedAt: 100,
    updatedAt: 200,
    completedAt: null,
    libraryId: "artists",
    libraryName: "Artists",
    libraryLabel: "Artists",
    progress: { determinate: true, percent: 42, label: "42 / 100" },
    scanPaths: [],
    sourceOperations: [],
    ...overrides
  };
}

describe("notification popover anchor geometry", () => {
  it("right-aligns to the button at an 800px viewport", () => {
    assert.deepEqual(
      resolveNotificationPopoverPosition({
        anchor: { left: 740, top: 10, right: 768, bottom: 38, width: 28, height: 28 },
        popover: { width: 360, height: 420 },
        viewport: { width: 800, height: 700 },
        gap: 4,
        margin: 8
      }),
      { left: 408, top: 42, maxHeight: 650 }
    );
  });

  it("clamps a narrow viewport without detaching vertically from the button", () => {
    assert.deepEqual(
      resolveNotificationPopoverPosition({
        anchor: { left: 276, top: 10, right: 304, bottom: 38, width: 28, height: 28 },
        popover: { width: 304, height: 500 },
        viewport: { width: 320, height: 600 },
        gap: 4,
        margin: 8
      }),
      { left: 8, top: 42, maxHeight: 550 }
    );
  });
});

describe("notification popover content presentation", () => {
  it("keeps phase, percentage, and current-total visible together", () => {
    assert.deepEqual(activeOperationPresentation(operation()), {
      phase: "索引文件",
      progressLabel: "42 / 100",
      percentLabel: "42%"
    });
  });

  it("always leads terminal detail with its library label", () => {
    assert.deepEqual(
      resultOperationPresentation(
        operation({ status: "failed", errorDetail: "读取失败", retryable: true })
      ),
      { libraryLabel: "Artists", detail: "读取失败" }
    );
    assert.deepEqual(
      resultOperationPresentation(operation({ status: "succeeded", summary: "完成" })),
      { libraryLabel: "Artists", detail: "完成" }
    );
  });
});

interface FakeElementState {
  rect: DOMRect;
  titlebar: Element | null;
}

class FakeEventTarget {
  readonly listeners = new Map<string, Set<() => void>>();

  addEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    const callback = listener as () => void;
    const listeners = this.listeners.get(type) ?? new Set<() => void>();
    listeners.add(callback);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    this.listeners.get(type)?.delete(listener as () => void);
  }

  emit(type: string) {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }
}

class FakeResizeObserver {
  readonly observed = new Set<Element>();
  disconnected = false;
  private readonly callback: () => void;

  constructor(callback: () => void) {
    this.callback = callback;
  }

  observe(target: Element) {
    this.observed.add(target);
  }

  unobserve(target: Element) {
    this.observed.delete(target);
  }

  disconnect() {
    this.disconnected = true;
    this.observed.clear();
  }

  emit(target: Element) {
    if (!this.disconnected && this.observed.has(target)) this.callback();
  }
}

class FakeMutationObserver {
  target: Element | null = null;
  options: MutationObserverInit | null = null;
  disconnected = false;
  private readonly callback: () => void;

  constructor(callback: () => void) {
    this.callback = callback;
  }

  observe(target: Element, options: MutationObserverInit) {
    this.target = target;
    this.options = options;
    this.disconnected = false;
  }

  disconnect() {
    this.disconnected = true;
    this.target = null;
  }

  emit(target: Element, type: MutationRecordType) {
    if (this.disconnected || target !== this.target) return;
    if (type === "attributes" && !this.options?.attributes) return;
    if (type === "childList" && !this.options?.childList) return;
    this.callback();
  }
}

function fakeRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    bottom: top + height,
    height,
    left,
    right: left + width,
    top,
    width,
    x: left,
    y: top,
    toJSON: () => ({})
  };
}

function fakeElement(state: FakeElementState): Element {
  return {
    getBoundingClientRect: () => state.rect,
    closest: () => state.titlebar,
    parentElement: null
  } as unknown as Element;
}

function measurementHarness() {
  const windowTarget = new FakeEventTarget();
  const viewportTarget = new FakeEventTarget();
  const resizeObservers: FakeResizeObserver[] = [];
  const mutationObservers: FakeMutationObserver[] = [];
  const frames = new Map<number, () => void>();
  let nextFrame = 1;
  const environment: NotificationPopoverMeasurementEnvironment = {
    cancelAnimationFrame: (handle) => frames.delete(handle),
    createMutationObserver: (callback) => {
      const observer = new FakeMutationObserver(callback);
      mutationObservers.push(observer);
      return observer;
    },
    createResizeObserver: (callback) => {
      const observer = new FakeResizeObserver(callback);
      resizeObservers.push(observer);
      return observer;
    },
    getViewport: () => ({ width: 800, height: 700 }),
    requestAnimationFrame: (callback) => {
      const handle = nextFrame++;
      frames.set(handle, callback);
      return handle;
    },
    viewportTarget,
    windowTarget
  };
  return {
    environment,
    flushFrames() {
      while (frames.size > 0) {
        const pending = [...frames.values()];
        frames.clear();
        for (const callback of pending) callback();
      }
    },
    mutationObservers,
    resizeObservers,
    viewportTarget,
    windowTarget
  };
}

describe("notification popover measurement lifecycle", () => {
  it("coalesces structural updates and ignores pointer style churn", () => {
    const harness = measurementHarness();
    const titlebar = fakeElement({ rect: fakeRect(0, 0, 800, 50), titlebar: null });
    const anchor = fakeElement({ rect: fakeRect(740, 10, 28, 28), titlebar });
    const popover = fakeElement({ rect: fakeRect(408, 42, 360, 420), titlebar: null });
    let measurementCount = 0;
    const stop = observeNotificationPopoverMeasurements({
      anchor,
      environment: harness.environment,
      onMeasure: () => {
        measurementCount += 1;
      },
      popover
    });

    harness.flushFrames();
    assert.equal(measurementCount, 1);
    assert.deepEqual(harness.mutationObservers[0]?.options, {
      childList: true,
      subtree: true
    });

    for (let index = 0; index < 120; index += 1) {
      harness.mutationObservers[0]?.emit(titlebar, "attributes");
    }
    harness.flushFrames();
    assert.equal(measurementCount, 1);

    for (let index = 0; index < 12; index += 1) {
      harness.mutationObservers[0]?.emit(titlebar, "childList");
    }
    harness.flushFrames();
    assert.equal(measurementCount, 2);
    stop();
  });

  it("rebinds a relocated titlebar and ignores the detached titlebar", () => {
    const harness = measurementHarness();
    const oldTitlebar = fakeElement({ rect: fakeRect(0, 0, 800, 50), titlebar: null });
    const nextTitlebar = fakeElement({ rect: fakeRect(0, 0, 800, 50), titlebar: null });
    const anchorState = { rect: fakeRect(740, 10, 28, 28), titlebar: oldTitlebar };
    const anchor = fakeElement(anchorState);
    const popover = fakeElement({ rect: fakeRect(408, 42, 360, 420), titlebar: null });
    let measurementCount = 0;
    const stop = observeNotificationPopoverMeasurements({
      anchor,
      environment: harness.environment,
      onMeasure: () => {
        measurementCount += 1;
      },
      popover
    });
    harness.flushFrames();

    anchorState.titlebar = nextTitlebar;
    harness.mutationObservers[0]?.emit(oldTitlebar, "childList");
    harness.flushFrames();
    assert.equal(measurementCount, 2);
    assert.equal(harness.resizeObservers[0]?.observed.has(oldTitlebar), false);
    assert.equal(harness.resizeObservers[0]?.observed.has(nextTitlebar), true);

    harness.mutationObservers[0]?.emit(oldTitlebar, "childList");
    harness.resizeObservers[0]?.emit(oldTitlebar);
    harness.flushFrames();
    assert.equal(measurementCount, 2);
    stop();
  });

  it("disconnects a replaced anchor before the new anchor measures", () => {
    const oldHarness = measurementHarness();
    const titlebar = fakeElement({ rect: fakeRect(0, 0, 800, 50), titlebar: null });
    const oldAnchor = fakeElement({ rect: fakeRect(740, 10, 28, 28), titlebar });
    const popover = fakeElement({ rect: fakeRect(408, 42, 360, 420), titlebar: null });
    let measurementCount = 0;
    const stopOld = observeNotificationPopoverMeasurements({
      anchor: oldAnchor,
      environment: oldHarness.environment,
      onMeasure: () => {
        measurementCount += 1;
      },
      popover
    });
    oldHarness.flushFrames();
    stopOld();

    oldHarness.resizeObservers[0]?.emit(oldAnchor);
    oldHarness.windowTarget.emit("resize");
    oldHarness.flushFrames();
    assert.equal(measurementCount, 1);

    const nextHarness = measurementHarness();
    const nextAnchor = fakeElement({ rect: fakeRect(500, 10, 28, 28), titlebar });
    const stopNext = observeNotificationPopoverMeasurements({
      anchor: nextAnchor,
      environment: nextHarness.environment,
      onMeasure: () => {
        measurementCount += 1;
      },
      popover
    });
    nextHarness.flushFrames();
    assert.equal(measurementCount, 2);
    stopNext();
  });
});
