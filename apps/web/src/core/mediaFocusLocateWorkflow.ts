export interface CancellableMediaFocusLocateStages {
  resolveTarget: () => Promise<boolean>;
  resolvePosition: () => Promise<number>;
  resetFilters: () => void;
  loadLibrary: () => Promise<void>;
  revealFolderPath: () => Promise<boolean>;
  loadMediaWindow: (position: number) => Promise<void>;
  selectMedia: () => void;
}

export async function runCoordinatedMediaFocusSwitch({
  libraryId,
  requestId,
  signal,
  switchLibrary,
  updateStage
}: {
  libraryId: string;
  requestId: string;
  signal: AbortSignal;
  switchLibrary: (libraryId: string, mediaFocusRequestId?: string) => Promise<{ ok: boolean }>;
  updateStage: (update: { requestId: string; stage: "switching" }) => Promise<boolean>;
}): Promise<boolean> {
  if (signal.aborted) return false;
  const accepted = await updateStage({ requestId, stage: "switching" });
  if (!accepted || signal.aborted) return false;
  const result = await switchLibrary(libraryId, requestId);
  return result.ok && !signal.aborted;
}

export async function runCancellableMediaFocusLocate(
  stages: CancellableMediaFocusLocateStages,
  signal: AbortSignal
): Promise<boolean> {
  if (signal.aborted || !(await stages.resolveTarget()) || signal.aborted) return false;
  const position = await stages.resolvePosition();
  if (signal.aborted) return false;
  stages.resetFilters();
  if (signal.aborted) return false;
  await stages.loadLibrary();
  if (signal.aborted) return false;
  if (!(await stages.revealFolderPath()) || signal.aborted) return false;
  await stages.loadMediaWindow(position);
  if (signal.aborted) return false;
  stages.selectMedia();
  return true;
}
