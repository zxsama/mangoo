export interface SecondaryWindowRevealState<T> {
  nativeReady: boolean;
  rendererReady: boolean;
  readonly result: Promise<T>;
  readonly cancellation: Promise<void>;
  readonly settled: boolean;
  readonly value: T | undefined;
  readonly warmupInFlight: boolean;
  beginWarmup(): number | null;
  isWarmupCurrent(token: number): boolean;
  finishWarmup(token: number): void;
  settle(value: T): boolean;
}

export const MATERIAL_WARMUP_HOLD_MS = 100;

export function createSecondaryWindowRevealState<T>(): SecondaryWindowRevealState<T> {
  let resolveResult!: (value: T) => void;
  let resolveCancellation!: () => void;
  const result = new Promise<T>((resolve) => {
    resolveResult = resolve;
  });
  const cancellation = new Promise<void>((resolve) => {
    resolveCancellation = resolve;
  });
  let settled = false;
  let value: T | undefined;
  let warmupInFlight = false;
  let warmupGeneration = 0;

  return {
    nativeReady: false,
    rendererReady: false,
    result,
    cancellation,
    get settled() {
      return settled;
    },
    get value() {
      return value;
    },
    get warmupInFlight() {
      return warmupInFlight;
    },
    beginWarmup() {
      if (settled || warmupInFlight || !this.nativeReady || !this.rendererReady) {
        return null;
      }
      warmupInFlight = true;
      warmupGeneration += 1;
      return warmupGeneration;
    },
    isWarmupCurrent(token) {
      return !settled && warmupInFlight && warmupGeneration === token;
    },
    finishWarmup(token) {
      if (!settled && warmupGeneration === token) {
        warmupInFlight = false;
      }
    },
    settle(nextValue) {
      if (settled) return false;
      settled = true;
      value = nextValue;
      warmupInFlight = false;
      warmupGeneration += 1;
      resolveCancellation();
      resolveResult(nextValue);
      return true;
    }
  };
}

export async function waitForSecondaryMaterialWarmup<T>(
  state: SecondaryWindowRevealState<T>,
  token: number,
  compositionFrame: Promise<void>,
  holdMs = MATERIAL_WARMUP_HOLD_MS
): Promise<boolean> {
  const frameOutcome = await Promise.race([
    compositionFrame.then(() => "ready" as const),
    state.cancellation.then(() => "cancelled" as const)
  ]);
  if (frameOutcome !== "ready" || !state.isWarmupCurrent(token)) return false;

  return new Promise<boolean>((resolve) => {
    let finished = false;
    let timer: NodeJS.Timeout;
    const finish = (didHold: boolean) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      resolve(didHold && state.isWarmupCurrent(token));
    };
    timer = setTimeout(() => finish(true), holdMs);
    void state.cancellation.then(() => finish(false));
  });
}
