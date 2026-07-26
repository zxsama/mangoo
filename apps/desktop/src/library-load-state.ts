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

export async function prepareColdLibraryStartup<Session>(options: {
  ensureCoreReady: () => Promise<Session>;
  prepareLibrary: (session: Session) => Promise<void>;
  publishPhase: (phase: DesktopLibraryLoadPhase) => void;
}): Promise<Session> {
  const session = await options.ensureCoreReady();
  await options.prepareLibrary(session);
  options.publishPhase("loading-media");
  return session;
}
