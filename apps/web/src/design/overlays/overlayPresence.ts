export type OverlayPresencePhase = "closed" | "opening" | "open" | "closing";
export type OverlayMotionKind = "root" | "nested" | "modal";

export interface OverlayPresenceState {
  present: boolean;
  phase: OverlayPresencePhase;
}

export type OverlayPresenceAction =
  | { type: "sync"; open: boolean; reducedMotion: boolean }
  | { type: "entered" }
  | { type: "exited" };

export function overlayMotionDuration(kind: OverlayMotionKind): number {
  if (kind === "nested") return 110;
  if (kind === "modal") return 160;
  return 140;
}

export function createOverlayPresenceState(
  open: boolean,
  reducedMotion: boolean
): OverlayPresenceState {
  if (!open) return { present: false, phase: "closed" };
  return { present: true, phase: reducedMotion ? "open" : "opening" };
}

export function reduceOverlayPresence(
  state: OverlayPresenceState,
  action: OverlayPresenceAction
): OverlayPresenceState {
  if (action.type === "entered") {
    return state.phase === "opening" ? { present: true, phase: "open" } : state;
  }
  if (action.type === "exited") {
    return state.phase === "closing" ? { present: false, phase: "closed" } : state;
  }
  if (action.open) {
    if (action.reducedMotion) return { present: true, phase: "open" };
    if (!state.present || state.phase === "closing") {
      return { present: true, phase: "opening" };
    }
    return state;
  }
  if (action.reducedMotion || !state.present) {
    return { present: false, phase: "closed" };
  }
  return state.phase === "closing" ? state : { present: true, phase: "closing" };
}
