export interface LibrarySummary {
  id: string;
  name: string;
  dbPath: string;
  available: boolean;
  active: boolean;
  // Optional visual identity (preset Lucide icon name + color) shown left of the
  // library name; absent → a default icon/color.
  icon?: string;
  color?: string;
}

export type LibraryLoadPhase =
  | "starting-core"
  | "opening-library"
  | "loading-media"
  | "ready"
  | { kind: "error"; message: string };

export function isErrorPhase(
  phase: LibraryLoadPhase
): phase is { kind: "error"; message: string } {
  return typeof phase === "object" && phase.kind === "error";
}
