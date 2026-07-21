export interface NotificationPopoverViewport {
  width: number;
  height: number;
}

export interface NotificationPopoverMeasurement {
  anchor: DOMRect;
  popover: DOMRect;
  viewport: NotificationPopoverViewport;
}

interface NotificationMeasurementEventTarget {
  addEventListener(type: string, listener: () => void, options?: boolean): void;
  removeEventListener(type: string, listener: () => void, options?: boolean): void;
}

interface NotificationMeasurementResizeObserver {
  observe(target: Element): void;
  unobserve(target: Element): void;
  disconnect(): void;
}

interface NotificationMeasurementMutationObserver {
  observe(target: Element, options: MutationObserverInit): void;
  disconnect(): void;
}

export interface NotificationPopoverMeasurementEnvironment {
  requestAnimationFrame(callback: () => void): number;
  cancelAnimationFrame(handle: number): void;
  createResizeObserver(callback: () => void): NotificationMeasurementResizeObserver;
  createMutationObserver(callback: () => void): NotificationMeasurementMutationObserver;
  getViewport(): NotificationPopoverViewport;
  windowTarget: NotificationMeasurementEventTarget;
  viewportTarget: NotificationMeasurementEventTarget | null;
}

export function observeNotificationPopoverMeasurements(input: {
  anchor: Element;
  popover: Element;
  onMeasure: (measurement: NotificationPopoverMeasurement) => void;
  environment?: NotificationPopoverMeasurementEnvironment;
}): () => void {
  const { anchor, popover, onMeasure } = input;
  const environment = input.environment ?? browserMeasurementEnvironment();
  let animationFrame = 0;
  let disposed = false;
  let titlebar: Element | null = null;

  const scheduleMeasurement = () => {
    if (disposed || animationFrame) return;
    animationFrame = environment.requestAnimationFrame(() => {
      animationFrame = 0;
      if (disposed) return;
      onMeasure({
        anchor: anchor.getBoundingClientRect(),
        popover: popover.getBoundingClientRect(),
        viewport: environment.getViewport()
      });
    });
  };

  const resizeObserver = environment.createResizeObserver(scheduleMeasurement);
  const mutationObserver = environment.createMutationObserver(() => {
    bindTitlebar();
    scheduleMeasurement();
  });

  const bindTitlebar = () => {
    const nextTitlebar = anchor.closest(".shell-titlebar") ?? anchor.parentElement;
    if (nextTitlebar === titlebar) return;
    if (titlebar) resizeObserver.unobserve(titlebar);
    mutationObserver.disconnect();
    titlebar = nextTitlebar;
    if (!titlebar) return;
    resizeObserver.observe(titlebar);
    mutationObserver.observe(titlebar, { childList: true, subtree: true });
  };

  resizeObserver.observe(anchor);
  resizeObserver.observe(popover);
  bindTitlebar();
  environment.windowTarget.addEventListener("resize", scheduleMeasurement);
  environment.windowTarget.addEventListener("scroll", scheduleMeasurement, true);
  environment.viewportTarget?.addEventListener("resize", scheduleMeasurement);
  environment.viewportTarget?.addEventListener("scroll", scheduleMeasurement);
  scheduleMeasurement();

  return () => {
    disposed = true;
    if (animationFrame) environment.cancelAnimationFrame(animationFrame);
    resizeObserver.disconnect();
    mutationObserver.disconnect();
    environment.windowTarget.removeEventListener("resize", scheduleMeasurement);
    environment.windowTarget.removeEventListener("scroll", scheduleMeasurement, true);
    environment.viewportTarget?.removeEventListener("resize", scheduleMeasurement);
    environment.viewportTarget?.removeEventListener("scroll", scheduleMeasurement);
  };
}

function browserMeasurementEnvironment(): NotificationPopoverMeasurementEnvironment {
  const viewport = window.visualViewport;
  return {
    cancelAnimationFrame: (handle) => window.cancelAnimationFrame(handle),
    createMutationObserver: (callback) => new MutationObserver(callback),
    createResizeObserver: (callback) => new ResizeObserver(callback),
    getViewport: () => ({
      width: viewport?.width ?? window.innerWidth,
      height: viewport?.height ?? window.innerHeight
    }),
    requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
    viewportTarget: viewport
      ? {
          addEventListener: (type, listener) => viewport.addEventListener(type, listener),
          removeEventListener: (type, listener) => viewport.removeEventListener(type, listener)
        }
      : null,
    windowTarget: {
      addEventListener: (type, listener, options) =>
        window.addEventListener(type, listener, options),
      removeEventListener: (type, listener, options) =>
        window.removeEventListener(type, listener, options)
    }
  };
}
