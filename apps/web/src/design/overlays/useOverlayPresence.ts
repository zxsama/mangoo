import { useEffect, useReducer, useState } from "react";
import {
  createOverlayPresenceState,
  overlayMotionDuration,
  reduceOverlayPresence,
  type OverlayMotionKind,
  type OverlayPresenceState
} from "./overlayPresence";

function reducedMotionPreferred(): boolean {
  return typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function useOverlayPresence(
  open: boolean,
  kind: OverlayMotionKind
): OverlayPresenceState {
  const [reducedMotion, setReducedMotion] = useState(reducedMotionPreferred);
  const [presence, dispatch] = useReducer(
    reduceOverlayPresence,
    { open, reducedMotion },
    ({ open: initialOpen, reducedMotion: initialReducedMotion }) =>
      createOverlayPresenceState(initialOpen, initialReducedMotion)
  );

  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    dispatch({ type: "sync", open, reducedMotion });
  }, [open, reducedMotion]);

  useEffect(() => {
    if (presence.phase !== "opening" && presence.phase !== "closing") return;
    const timer = window.setTimeout(() => {
      dispatch({ type: presence.phase === "opening" ? "entered" : "exited" });
    }, overlayMotionDuration(kind));
    return () => window.clearTimeout(timer);
  }, [kind, presence.phase]);

  return presence;
}
