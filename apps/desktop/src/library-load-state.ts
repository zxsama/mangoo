export type DesktopLibraryLoadPhase =
  | "starting-core"
  | "opening-library"
  | "loading-media"
  | "ready"
  | { kind: "error"; message: string };

export class LibraryLoadState {
  private current: DesktopLibraryLoadPhase = "starting-core";

  publish(phase: DesktopLibraryLoadPhase): void {
    this.current = phase;
  }

  snapshot(): DesktopLibraryLoadPhase {
    return this.current;
  }
}
