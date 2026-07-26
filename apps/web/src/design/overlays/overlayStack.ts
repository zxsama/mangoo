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
  return {
    ...state,
    layers: [...state.layers.filter((item) => item.id !== layer.id), layer]
  };
}

export function popOverlayLayer(state: OverlayStackState, id: string): OverlayStackState {
  return { ...state, layers: state.layers.filter((layer) => layer.id !== id) };
}

export function topOverlayLayer(state: OverlayStackState): OverlayLayerRegistration | null {
  return state.layers.at(-1) ?? null;
}
