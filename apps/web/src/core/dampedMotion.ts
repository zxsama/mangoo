export interface DampedMotionAxisOptions {
  acceleration?: number;
  accelerationDelta?: number;
  friction?: number;
  maxStep?: number;
  stopThreshold?: number;
}

const DEFAULT_FRICTION = 0.84;
const DEFAULT_ACCELERATION = 0;
const DEFAULT_ACCELERATION_DELTA = 480;
const DEFAULT_MAX_STEP = 900;
const DEFAULT_STOP_THRESHOLD = 0.1;
const REFERENCE_FRAME_MS = 1000 / 60;
const MAX_FRAME_UNITS = 4;

export class DampedMotionAxis {
  private acceleration: number;
  private accelerationDelta: number;
  private friction: number;
  private inputGain: number;
  private maxStep: number;
  private stopThreshold: number;
  private velocity = 0;

  constructor(options: DampedMotionAxisOptions = {}) {
    this.acceleration = Math.max(0, options.acceleration ?? DEFAULT_ACCELERATION);
    this.accelerationDelta = Math.max(1, options.accelerationDelta ?? DEFAULT_ACCELERATION_DELTA);
    this.friction = clamp(options.friction ?? DEFAULT_FRICTION, 0.01, 0.98);
    this.inputGain = 1 - this.friction;
    this.maxStep = Math.max(1, options.maxStep ?? DEFAULT_MAX_STEP);
    this.stopThreshold = Math.max(0.001, options.stopThreshold ?? DEFAULT_STOP_THRESHOLD);
  }

  get active(): boolean {
    return Math.abs(this.velocity) >= this.stopThreshold;
  }

  push(delta: number): void {
    if (!Number.isFinite(delta) || delta === 0) {
      return;
    }
    const inputGain = this.acceleratedInputGain(delta);
    this.velocity = clamp(
      this.velocity + delta * inputGain,
      -this.maxStep,
      this.maxStep
    );
  }

  step(elapsedMs = REFERENCE_FRAME_MS): number {
    if (!this.active) {
      this.velocity = 0;
      return 0;
    }
    const frameUnits = normalizeFrameUnits(elapsedMs);
    const decay = Math.pow(this.friction, frameUnits);
    const output = this.velocity * ((1 - decay) / (1 - this.friction));
    this.velocity *= decay;
    if (Math.abs(this.velocity) < this.stopThreshold) {
      this.velocity = 0;
    }
    return output;
  }

  stop(): void {
    this.velocity = 0;
  }

  private acceleratedInputGain(delta: number): number {
    if (this.acceleration <= 0) {
      return this.inputGain;
    }
    const intensity = clamp(Math.abs(delta) / this.accelerationDelta, 0, 1);
    return this.inputGain * (1 + this.acceleration * intensity);
  }
}

export function normalizeWheelDelta(
  delta: number,
  deltaMode: number,
  lineHeightPx = 16,
  pageSizePx = 800
): number {
  if (!Number.isFinite(delta) || delta === 0) {
    return 0;
  }
  if (deltaMode === 1) {
    return delta * lineHeightPx;
  }
  if (deltaMode === 2) {
    return delta * pageSizePx;
  }
  return delta;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function normalizeFrameUnits(elapsedMs: number): number {
  if (!Number.isFinite(elapsedMs) || elapsedMs <= 0) {
    return 1;
  }
  return clamp(elapsedMs / REFERENCE_FRAME_MS, 0.25, MAX_FRAME_UNITS);
}
