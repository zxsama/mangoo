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
  closeTransientOverlayLayers,
  closeOverlayRoot,
  createOverlayStackState,
  openOverlayRoot,
  popOverlayLayer,
  pushOverlayLayer,
  topOverlayLayer,
  unregisterOverlayLayer,
  type OverlayStackState
} from "./overlayStack";
import {
  bindOverlayDismissalListeners,
  resolveOverlayFocusRestoreTarget,
  shouldDismissOverlayLayer,
  shouldRestoreOverlayFocus,
  type OverlayDismissReason
} from "./overlayRuntime";
import type {
  OverlayLayerRegistration,
  OverlayRootId,
  OverlayStackApi
} from "./overlayTypes";

const OverlayStackContext = createContext<OverlayStackApi | null>(null);

interface OverlayFamilyFocusApi {
  isFocusOwnedByLayerAbove: (ownerSurface: HTMLElement, target: Node | null) => boolean;
}

const OverlayFamilyFocusContext = createContext<OverlayFamilyFocusApi | null>(null);

function targetBelongsToLayer(layer: OverlayLayerRegistration, target: EventTarget | null): boolean {
  if (!(target instanceof Node)) return false;
  return Boolean(layer.surface?.contains(target) || layer.trigger?.contains(target));
}

function restoreLayerFocus(layer: OverlayLayerRegistration, reason: OverlayDismissReason): void {
  if (!shouldRestoreOverlayFocus(layer, reason)) return;
  resolveOverlayFocusRestoreTarget(layer.trigger, null)?.focus();
}

export function OverlayProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<OverlayStackState>(createOverlayStackState);
  const stateRef = useRef(state);

  const commitState = useCallback((next: OverlayStackState) => {
    stateRef.current = next;
    setState(next);
  }, []);

  const dismissLayer = useCallback((
    layer: OverlayLayerRegistration,
    reason: OverlayDismissReason
  ): boolean => {
    const current = stateRef.current;
    if (!current.layers.some((item) => item.id === layer.id)) return false;

    let next = popOverlayLayer(current, layer.id);
    if (next.layers.length === 0) {
      next = closeOverlayRoot(next);
    }
    commitState(next);
    layer.onDismiss();
    if (stateRef.current === next) {
      restoreLayerFocus(layer, reason);
    }
    return true;
  }, [commitState]);

  const openRoot = useCallback((rootId: OverlayRootId) => {
    const current = stateRef.current;
    if (current.activeRootId === rootId) return;

    const previousLayers = [...current.layers].reverse();
    commitState(openOverlayRoot(current, rootId));
    for (const layer of previousLayers) {
      layer.onDismiss();
    }
  }, [commitState]);

  const closeRootFamily = useCallback((
    rootId: OverlayRootId | undefined,
    reason: OverlayDismissReason
  ) => {
    const current = stateRef.current;
    if (
      current.activeRootId === null ||
      (rootId !== undefined && current.activeRootId !== rootId)
    ) {
      return;
    }

    const rootLayer = current.layers.find((layer) => layer.kind === "root") ?? null;
    const next = closeOverlayRoot(current, rootId);
    commitState(next);
    for (const layer of [...current.layers].reverse()) {
      layer.onDismiss();
    }
    if (rootLayer && stateRef.current === next) {
      restoreLayerFocus(rootLayer, reason);
    }
  }, [commitState]);

  const closeRoot = useCallback((rootId?: OverlayRootId) => {
    closeRootFamily(rootId, "programmatic");
  }, [closeRootFamily]);

  const registerLayer = useCallback((layer: OverlayLayerRegistration) => {
    let next = stateRef.current;
    if (next.activeRootId === null) {
      if (layer.kind === "nested") {
        layer.onDismiss();
        return () => undefined;
      }
      next = openOverlayRoot(next, layer.rootId);
    }
    const registeredState = pushOverlayLayer(next, layer);
    if (registeredState === next) {
      layer.onDismiss();
      return () => undefined;
    }
    commitState(registeredState);

    return () => {
      queueMicrotask(() => {
        const current = stateRef.current;
        const registered = current.layers.find((item) => item.id === layer.id);
        if (registered !== layer) return;

        if (layer.kind === "root") {
          const descendants = current.layers.filter((item) => item !== layer).reverse();
          const cleaned = closeOverlayRoot(current, layer.rootId);
          if (cleaned === current) return;
          commitState(cleaned);
          for (const descendant of descendants) {
            descendant.onDismiss();
          }
          if (stateRef.current === cleaned) {
            restoreLayerFocus(layer, "programmatic");
          }
          return;
        }

        let cleaned = unregisterOverlayLayer(current, layer);
        if (cleaned.layers.length === 0) {
          cleaned = closeOverlayRoot(cleaned);
        }
        commitState(cleaned);
        restoreLayerFocus(layer, "programmatic");
      });
    };
  }, [commitState]);

  const dismissTop = useCallback((): boolean => {
    const layer = topOverlayLayer(stateRef.current);
    return layer ? dismissLayer(layer, "programmatic") : false;
  }, [dismissLayer]);

  const hasLayers = state.layers.length > 0;

  useEffect(() => {
    if (!hasLayers) return;

    return bindOverlayDismissalListeners({
      documentTarget: document,
      windowTarget: window,
      getVisibilityState: () => document.visibilityState,
      onDismissRequest: (reason, event) => {
        const current = stateRef.current;
        const layer = topOverlayLayer(current);
        if (!layer) return false;

        if (
          reason === "window-blur" ||
          reason === "pagehide" ||
          reason === "visibility-hidden"
        ) {
          const transientLayers = current.layers.filter((item) => item.dismissOnWindowBlur);
          if (transientLayers.length === 0) return false;
          commitState(closeTransientOverlayLayers(current));
          for (const transientLayer of [...transientLayers].reverse()) {
            transientLayer.onDismiss();
          }
          return true;
        }

        const targetBelongs = targetBelongsToLayer(layer, event.target);
        if (!shouldDismissOverlayLayer(layer, reason, targetBelongs)) return false;
        return dismissLayer(layer, reason);
      }
    });
  }, [commitState, dismissLayer, hasLayers]);

  const api = useMemo<OverlayStackApi>(() => ({
    activeRootId: state.activeRootId,
    openRoot,
    closeRoot,
    isRootOpen: (rootId) => state.activeRootId === rootId,
    registerLayer,
    dismissTop
  }), [closeRoot, dismissTop, openRoot, registerLayer, state.activeRootId]);

  const familyFocusApi = useMemo<OverlayFamilyFocusApi>(() => ({
    isFocusOwnedByLayerAbove: (ownerSurface, target) => {
      if (target === null) return false;
      const current = stateRef.current;
      const ownerIndex = current.layers.findIndex(
        (layer) => layer.surface === ownerSurface
      );
      if (ownerIndex < 0) return false;
      const owner = current.layers[ownerIndex];
      return current.layers.slice(ownerIndex + 1).some((layer) => (
        layer.rootId === owner.rootId && Boolean(layer.surface?.contains(target))
      ));
    }
  }), []);

  return (
    <OverlayFamilyFocusContext.Provider value={familyFocusApi}>
      <OverlayStackContext.Provider value={api}>
        {children}
      </OverlayStackContext.Provider>
    </OverlayFamilyFocusContext.Provider>
  );
}

export function useOverlayStack(): OverlayStackApi {
  const value = useContext(OverlayStackContext);
  if (!value) {
    throw new Error("useOverlayStack must be used within OverlayProvider");
  }
  return value;
}

export function useOverlayFamilyFocus(): OverlayFamilyFocusApi {
  const value = useContext(OverlayFamilyFocusContext);
  if (!value) {
    throw new Error("useOverlayFamilyFocus must be used within OverlayProvider");
  }
  return value;
}
