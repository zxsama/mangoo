export type {
  ActiveLibraryResponse,
  CreateLibraryRequest,
  CreateLibraryResponse,
  LibraryAvailability,
  LibraryBindingState,
  LibraryLifecycleState,
  LibraryListResponse,
  LibraryPreflightReason,
  LibraryPreflightRequest,
  LibraryPreflightResponse,
  LibrarySummary,
} from "@mangoo/core-client";

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
