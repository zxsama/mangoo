import type { OverlayLayerRegistration, OverlayRootId } from "./overlayTypes";

export interface OverlayStackState {
  activeRootId: OverlayRootId | null;
  layers: OverlayLayerRegistration[];
}

export const createOverlayStackState = (): OverlayStackState => ({
  activeRootId: null,
  layers: []
});

export function openOverlayRoot(state: OverlayStackState, rootId: OverlayRootId): OverlayStackState {
  if (state.activeRootId === rootId) return state;
  return { activeRootId: rootId, layers: [] };
}

export function closeOverlayRoot(
  state: OverlayStackState,
  rootId?: OverlayRootId
): OverlayStackState {
  if (state.activeRootId === null || (rootId !== undefined && state.activeRootId !== rootId)) {
    return state;
  }
  return createOverlayStackState();
}

export function pushOverlayLayer(
  state: OverlayStackState,
  layer: OverlayLayerRegistration
): OverlayStackState {
  const existingIndex = state.layers.findIndex((item) => item.id === layer.id);
  const existing = existingIndex >= 0 ? state.layers[existingIndex] : null;
  if (
    state.activeRootId !== layer.rootId ||
    (existing && (
      existing.rootId !== layer.rootId ||
      existing.kind !== layer.kind ||
      existing.parentId !== layer.parentId
    ))
  ) {
    return state;
  }

  const otherLayers = existingIndex >= 0
    ? state.layers.filter((_, index) => index !== existingIndex)
    : state.layers;
  if (layer.kind === "root") {
    if (layer.parentId !== undefined || otherLayers.some((item) => item.kind === "root")) {
      return state;
    }
  } else if (layer.kind === "nested") {
    const parent = layer.parentId
      ? otherLayers.find((item) => item.id === layer.parentId)
      : null;
    if (!parent || parent.rootId !== layer.rootId || parent.kind === "modal") {
      return state;
    }
    if (existingIndex < 0 && otherLayers.at(-1)?.id !== parent.id) {
      return state;
    }
  } else if (layer.parentId !== undefined) {
    const parent = otherLayers.find((item) => item.id === layer.parentId);
    if (!parent || parent.rootId !== layer.rootId) {
      return state;
    }
  }

  if (existingIndex >= 0) {
    const layers = [...state.layers];
    layers[existingIndex] = layer;
    return { ...state, layers };
  }
  return {
    ...state,
    layers: [...state.layers, layer]
  };
}

export function popOverlayLayer(state: OverlayStackState, id: string): OverlayStackState {
  return { ...state, layers: state.layers.filter((layer) => layer.id !== id) };
}

export function unregisterOverlayLayer(
  state: OverlayStackState,
  registration: OverlayLayerRegistration
): OverlayStackState {
  const registered = state.layers.find((layer) => layer.id === registration.id);
  return registered === registration ? popOverlayLayer(state, registration.id) : state;
}

export function closeTransientOverlayLayers(state: OverlayStackState): OverlayStackState {
  const layers = state.layers.filter((layer) => !layer.dismissOnWindowBlur);
  if (layers.length === state.layers.length) return state;
  return layers.length === 0 ? createOverlayStackState() : { ...state, layers };
}

export function topOverlayLayer(state: OverlayStackState): OverlayLayerRegistration | null {
  return state.layers.at(-1) ?? null;
}
