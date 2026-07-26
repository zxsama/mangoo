import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode
} from "react";
import {
  closeOverlayRoot,
  createOverlayStackState,
  openOverlayRoot,
  popOverlayLayer,
  pushOverlayLayer,
  topOverlayLayer,
  type OverlayStackState
} from "./overlayStack";
import type {
  OverlayLayerRegistration,
  OverlayRootId,
  OverlayStackApi
} from "./overlayTypes";

const OverlayStackContext = createContext<OverlayStackApi | null>(null);

function targetBelongsToLayer(layer: OverlayLayerRegistration, target: EventTarget | null): boolean {
  if (!(target instanceof Node)) return false;
  return Boolean(layer.surface?.contains(target) || layer.trigger?.contains(target));
}

function restoreLayerFocus(layer: OverlayLayerRegistration): void {
  if (layer.restoreFocus && layer.trigger?.isConnected) {
    layer.trigger?.focus();
  }
}

export function OverlayProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<OverlayStackState>(createOverlayStackState);
  const stateRef = useRef(state);

  const commitState = useCallback((next: OverlayStackState) => {
    stateRef.current = next;
    setState(next);
  }, []);

  const dismissLayer = useCallback((layer: OverlayLayerRegistration): boolean => {
    const current = stateRef.current;
    if (!current.layers.some((item) => item.id === layer.id)) return false;

    layer.onDismiss();
    let next = popOverlayLayer(current, layer.id);
    if (next.layers.length === 0) {
      next = closeOverlayRoot(next);
    }
    commitState(next);
    restoreLayerFocus(layer);
    return true;
  }, [commitState]);

  const openRoot = useCallback((rootId: OverlayRootId) => {
    const current = stateRef.current;
    if (current.activeRootId === rootId) return;

    const previousLayers = [...current.layers].reverse();
    for (const layer of previousLayers) {
      layer.onDismiss();
    }
    commitState(openOverlayRoot(current, rootId));
  }, [commitState]);

  const closeRoot = useCallback((rootId?: OverlayRootId) => {
    const current = stateRef.current;
    if (
      current.activeRootId === null ||
      (rootId !== undefined && current.activeRootId !== rootId)
    ) {
      return;
    }

    const rootLayer = current.layers.find((layer) => layer.kind === "root") ?? null;
    for (const layer of [...current.layers].reverse()) {
      layer.onDismiss();
    }
    commitState(closeOverlayRoot(current, rootId));
    if (rootLayer) restoreLayerFocus(rootLayer);
  }, [commitState]);

  const registerLayer = useCallback((layer: OverlayLayerRegistration) => {
    let next = stateRef.current;
    if (next.activeRootId === null) {
      next = openOverlayRoot(next, layer.rootId);
    }
    commitState(pushOverlayLayer(next, layer));

    return () => {
      const current = stateRef.current;
      const registered = current.layers.find((item) => item.id === layer.id);
      if (!registered) return;
      let cleaned = popOverlayLayer(current, layer.id);
      if (cleaned.layers.length === 0) {
        cleaned = closeOverlayRoot(cleaned);
      }
      commitState(cleaned);
    };
  }, [commitState]);

  const dismissTop = useCallback((): boolean => {
    const layer = topOverlayLayer(stateRef.current);
    return layer ? dismissLayer(layer) : false;
  }, [dismissLayer]);

  const hasLayers = state.layers.length > 0;

  useEffect(() => {
    if (!hasLayers) return;

    const handlePointerDown = (event: PointerEvent) => {
      const layer = topOverlayLayer(stateRef.current);
      if (
        layer?.dismissOnOutsidePointer &&
        !targetBelongsToLayer(layer, event.target)
      ) {
        dismissLayer(layer);
      }
    };

    const handleFocusIn = (event: FocusEvent) => {
      const layer = topOverlayLayer(stateRef.current);
      if (
        layer &&
        layer.kind !== "modal" &&
        layer.dismissOnFocusOutside &&
        !targetBelongsToLayer(layer, event.target)
      ) {
        dismissLayer(layer);
      }
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const layer = topOverlayLayer(stateRef.current);
      if (layer?.dismissOnEscape && dismissLayer(layer)) {
        event.preventDefault();
        event.stopPropagation();
      }
    };

    const handleWindowDeparture = () => {
      const layer = topOverlayLayer(stateRef.current);
      if (layer?.dismissOnWindowBlur) {
        closeRoot(stateRef.current.activeRootId ?? undefined);
      }
    };

    document.addEventListener("pointerdown", handlePointerDown, true);
    document.addEventListener("focusin", handleFocusIn, true);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener("blur", handleWindowDeparture);
    window.addEventListener("pagehide", handleWindowDeparture);

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown, true);
      document.removeEventListener("focusin", handleFocusIn, true);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("blur", handleWindowDeparture);
      window.removeEventListener("pagehide", handleWindowDeparture);
    };
  }, [closeRoot, dismissLayer, hasLayers]);

  const api = useMemo<OverlayStackApi>(() => ({
    activeRootId: state.activeRootId,
    openRoot,
    closeRoot,
    isRootOpen: (rootId) => state.activeRootId === rootId,
    registerLayer,
    dismissTop
  }), [closeRoot, dismissTop, openRoot, registerLayer, state.activeRootId]);

  return (
    <OverlayStackContext.Provider value={api}>
      {children}
    </OverlayStackContext.Provider>
  );
}

export function useOverlayStack(): OverlayStackApi {
  const value = useContext(OverlayStackContext);
  if (!value) {
    throw new Error("useOverlayStack must be used within OverlayProvider");
  }
  return value;
}
