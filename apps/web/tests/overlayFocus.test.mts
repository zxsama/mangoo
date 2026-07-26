import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

interface OverlayRuntimeApi {
  bindOverlayDismissalListeners(input: {
    documentTarget: EventTarget;
    windowTarget: EventTarget;
    getVisibilityState: () => DocumentVisibilityState;
    onDismissRequest: (reason: string, event: Event) => boolean;
  }): () => void;
  canRestoreOverlayFocus(input: {
    isConnected: boolean;
    matches: (selector: string) => boolean;
    getAttribute: (name: string) => string | null;
  }): boolean;
  resolveOverlayFocusRestoreTarget<T extends {
    isConnected: boolean;
    matches: (selector: string) => boolean;
    getAttribute: (name: string) => string | null;
  }>(requested: T | null, fallback: T | null): T | null;
  shouldDismissOverlayLayer(
    layer: ReturnType<typeof layer>,
    reason: string,
    targetBelongsToLayer?: boolean
  ): boolean;
  shouldRestoreOverlayFocus(layer: ReturnType<typeof layer>, reason: string): boolean;
}

const loadRuntime = async (): Promise<OverlayRuntimeApi> => {
  try {
    return await import("../src/design/overlays/overlayRuntime.ts") as OverlayRuntimeApi;
  } catch (error) {
    assert.fail(`overlay runtime seam is unavailable: ${String(error)}`);
  }
};

const layer = (kind: "root" | "nested" | "modal" = "root") => ({
  id: "layer",
  rootId: "library" as const,
  kind,
  trigger: null,
  surface: null,
  dismissOnEscape: true,
  dismissOnFocusOutside: true,
  dismissOnOutsidePointer: true,
  dismissOnWindowBlur: true,
  restoreFocus: true,
  onDismiss: () => undefined
});

const keyboardEvent = (key: string) => {
  const event = new Event("keydown", { cancelable: true });
  Object.defineProperty(event, "key", { value: key });
  return event;
};

const focusTarget = ({
  ariaDisabled = false,
  isConnected = true,
  nativeDisabled = false
}: {
  ariaDisabled?: boolean;
  isConnected?: boolean;
  nativeDisabled?: boolean;
} = {}) => ({
  isConnected,
  matches: (selector: string) => selector === ":disabled" && nativeDisabled,
  getAttribute: (name: string) => name === "aria-disabled" && ariaDisabled ? "true" : null
});

const dialogSource = readFileSync(
  new URL("../src/design/overlays/OverlayDialog.tsx", import.meta.url),
  "utf8"
);

describe("shared overlay focus and dismissal behavior", () => {
  it("honors top-layer dismissal flags and modal focus ownership", async () => {
    const runtime = await loadRuntime();
    const root = layer();
    assert.equal(runtime.shouldDismissOverlayLayer(root, "escape"), true);
    assert.equal(runtime.shouldDismissOverlayLayer(root, "outside-pointer", false), true);
    assert.equal(runtime.shouldDismissOverlayLayer(root, "outside-pointer", true), false);
    assert.equal(runtime.shouldDismissOverlayLayer(root, "focus-outside", false), true);
    assert.equal(runtime.shouldDismissOverlayLayer(layer("modal"), "focus-outside", false), false);
    assert.equal(runtime.shouldDismissOverlayLayer({ ...root, dismissOnEscape: false }, "escape"), false);
  });

  it("lets Tab continue outside without restoring the trigger", async () => {
    const runtime = await loadRuntime();
    assert.equal(runtime.shouldRestoreOverlayFocus(layer(), "focus-outside"), false);
    assert.equal(runtime.shouldRestoreOverlayFocus(layer(), "escape"), true);
    assert.equal(runtime.shouldRestoreOverlayFocus({ ...layer(), restoreFocus: false }, "escape"), false);
  });

  it("restores focus only to connected enabled triggers", async () => {
    const runtime = await loadRuntime();
    assert.equal(runtime.canRestoreOverlayFocus(focusTarget()), true);
    assert.equal(runtime.canRestoreOverlayFocus(focusTarget({ isConnected: false })), false);
    assert.equal(runtime.canRestoreOverlayFocus(focusTarget({ nativeDisabled: true })), false);
    assert.equal(runtime.canRestoreOverlayFocus(focusTarget({ ariaDisabled: true })), false);
  });

  it("falls back when a requested modal restore target is disabled", async () => {
    const runtime = await loadRuntime();
    assert.equal(typeof runtime.resolveOverlayFocusRestoreTarget, "function");
    const fallback = focusTarget();
    const requested = focusTarget();
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(requested, fallback), requested);
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(null, fallback), fallback);
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(
      focusTarget({ nativeDisabled: true }),
      fallback
    ), fallback);
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(
      focusTarget({ ariaDisabled: true }),
      fallback
    ), fallback);
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(
      focusTarget({ isConnected: false }),
      fallback
    ), fallback);
    assert.equal(runtime.resolveOverlayFocusRestoreTarget(
      focusTarget({ nativeDisabled: true }),
      focusTarget({ ariaDisabled: true })
    ), null);
  });

  it("keeps modal keyboard containment in the shared focus trap", () => {
    assert.match(dialogSource, /useOverlayFocusTrap\(/);
    assert.doesNotMatch(dialogSource, /document\.addEventListener\(["']keydown["']/);
  });

  it("delivers dismissal events once and removes every listener", async () => {
    const runtime = await loadRuntime();
    const documentTarget = new EventTarget();
    const windowTarget = new EventTarget();
    const reasons: string[] = [];
    let visibilityState: DocumentVisibilityState = "visible";
    const cleanup = runtime.bindOverlayDismissalListeners({
      documentTarget,
      windowTarget,
      getVisibilityState: () => visibilityState,
      onDismissRequest: (reason) => {
        reasons.push(reason);
        return true;
      }
    });

    const escape = keyboardEvent("Escape");
    documentTarget.dispatchEvent(escape);
    documentTarget.dispatchEvent(new Event("pointerdown"));
    documentTarget.dispatchEvent(new Event("focusin"));
    windowTarget.dispatchEvent(new Event("blur"));
    windowTarget.dispatchEvent(new Event("pagehide"));
    documentTarget.dispatchEvent(new Event("visibilitychange"));
    visibilityState = "hidden";
    documentTarget.dispatchEvent(new Event("visibilitychange"));

    assert.equal(escape.defaultPrevented, true);
    assert.deepEqual(reasons, [
      "escape",
      "outside-pointer",
      "focus-outside",
      "window-blur",
      "pagehide",
      "visibility-hidden"
    ]);

    cleanup();
    documentTarget.dispatchEvent(keyboardEvent("Escape"));
    windowTarget.dispatchEvent(new Event("blur"));
    assert.equal(reasons.length, 6);
  });
});
