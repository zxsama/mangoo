import type { OverlayLayerRegistration } from "./overlayTypes";

export type OverlayDismissReason =
  | "escape"
  | "outside-pointer"
  | "focus-outside"
  | "window-blur"
  | "pagehide"
  | "visibility-hidden"
  | "programmatic";

export interface OverlayFocusTargetState {
  isConnected: boolean;
  getAttribute(name: string): string | null;
  matches(selector: string): boolean;
}

export function shouldDismissOverlayLayer(
  layer: OverlayLayerRegistration,
  reason: OverlayDismissReason,
  targetBelongsToLayer = false
): boolean {
  switch (reason) {
    case "escape":
      return layer.dismissOnEscape;
    case "outside-pointer":
      return layer.dismissOnOutsidePointer && !targetBelongsToLayer;
    case "focus-outside":
      return layer.kind !== "modal" && layer.dismissOnFocusOutside && !targetBelongsToLayer;
    case "window-blur":
    case "pagehide":
    case "visibility-hidden":
      return layer.dismissOnWindowBlur;
    case "programmatic":
      return true;
  }
}

export function shouldRestoreOverlayFocus(
  layer: OverlayLayerRegistration,
  reason: OverlayDismissReason
): boolean {
  return layer.restoreFocus && reason !== "focus-outside";
}

export function canRestoreOverlayFocus(target: OverlayFocusTargetState): boolean {
  return target.isConnected &&
    !target.matches(":disabled") &&
    target.getAttribute("aria-disabled")?.toLocaleLowerCase() !== "true";
}

export function resolveOverlayFocusRestoreTarget<T extends OverlayFocusTargetState>(
  requested: T | null | undefined,
  fallback: T | null | undefined
): T | null {
  if (requested && canRestoreOverlayFocus(requested)) return requested;
  return fallback && canRestoreOverlayFocus(fallback) ? fallback : null;
}

export interface BindOverlayDismissalListenersInput {
  documentTarget: EventTarget;
  windowTarget: EventTarget;
  getVisibilityState: () => DocumentVisibilityState;
  onDismissRequest: (reason: OverlayDismissReason, event: Event) => boolean;
}

export function bindOverlayDismissalListeners({
  documentTarget,
  windowTarget,
  getVisibilityState,
  onDismissRequest
}: BindOverlayDismissalListenersInput): () => void {
  const handlePointerDown = (event: Event) => {
    onDismissRequest("outside-pointer", event);
  };
  const handleFocusIn = (event: Event) => {
    onDismissRequest("focus-outside", event);
  };
  const handleKeyDown = (event: Event) => {
    if ((event as KeyboardEvent).key !== "Escape") return;
    if (onDismissRequest("escape", event)) {
      event.preventDefault();
      event.stopPropagation();
    }
  };
  const handleWindowBlur = (event: Event) => {
    onDismissRequest("window-blur", event);
  };
  const handlePageHide = (event: Event) => {
    onDismissRequest("pagehide", event);
  };
  const handleVisibilityChange = (event: Event) => {
    if (getVisibilityState() === "hidden") {
      onDismissRequest("visibility-hidden", event);
    }
  };

  documentTarget.addEventListener("pointerdown", handlePointerDown, true);
  documentTarget.addEventListener("focusin", handleFocusIn, true);
  documentTarget.addEventListener("keydown", handleKeyDown);
  documentTarget.addEventListener("visibilitychange", handleVisibilityChange);
  windowTarget.addEventListener("blur", handleWindowBlur);
  windowTarget.addEventListener("pagehide", handlePageHide);

  return () => {
    documentTarget.removeEventListener("pointerdown", handlePointerDown, true);
    documentTarget.removeEventListener("focusin", handleFocusIn, true);
    documentTarget.removeEventListener("keydown", handleKeyDown);
    documentTarget.removeEventListener("visibilitychange", handleVisibilityChange);
    windowTarget.removeEventListener("blur", handleWindowBlur);
    windowTarget.removeEventListener("pagehide", handlePageHide);
  };
}
