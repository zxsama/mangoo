export function resolveFolderLocationScrollFrame({
  from,
  to,
  elapsedMs,
  durationMs
}: {
  from: number;
  to: number;
  elapsedMs: number;
  durationMs: number;
}): { done: boolean; top: number } {
  if (durationMs <= 0 || elapsedMs >= durationMs) {
    return { done: true, top: to };
  }

  const progress = Math.max(0, elapsedMs / durationMs);
  const easedProgress = 1 - Math.pow(1 - progress, 3);
  return {
    done: false,
    top: from + (to - from) * easedProgress
  };
}
