import { useCallback, useEffect, useMemo, useState } from "react";

export const INTERFACE_STYLE_STORAGE_KEY = "megle.interfaceStyle";

// Base cursor-proximity radius (px) at which an interactive affordance's circular
// glow first ignites. The `pointerSensingDistance` preference scales this base, and
// LiquidGlassSurface reads the resulting `--glass-pointer-sensing-distance` var per
// frame instead of hardcoding the radius.
export const POINTER_SENSING_DISTANCE_BASE_PX = 112;

export interface InterfaceStylePreference {
  windowCornerRadius: number;
  surfaceCornerRadius: number;
  controlCornerRadius: number;
  contentCornerRadius: number;
  sideBlur: number;
  sideOpacity: number;
  sideOverlayStrength: number;
  sideOverlayColor: string;
  sideSaturation: number;
  sideStrokeOpacity: number;
  centerBlur: number;
  centerOpacity: number;
  centerOverlayStrength: number;
  centerOverlayColor: string;
  centerSaturation: number;
  centerStrokeOpacity: number;
  ditherOpacity: number;
  backdropGradientStrength: number;
  edgeHighlightBrightness: number;
  edgeHighlightSize: number;
  haloBrightness: number;
  haloFalloff: number;
  pointerResponseRadius: number;
  pointerSensingDistance: number;
  refractionStrength: number;
  dialogBlur: number;
  dialogOpacity: number;
  dialogOverlayStrength: number;
  dialogBackdropDim: number;
}

export const DEFAULT_INTERFACE_STYLE: InterfaceStylePreference = {
  windowCornerRadius: 12,
  surfaceCornerRadius: 18,
  controlCornerRadius: 10,
  contentCornerRadius: 8,
  sideBlur: 1,
  sideOpacity: 1,
  sideOverlayStrength: 1,
  sideOverlayColor: "#080c10",
  sideSaturation: 1.55,
  sideStrokeOpacity: 1,
  centerBlur: 1,
  centerOpacity: 1,
  centerOverlayStrength: 1,
  centerOverlayColor: "#080c10",
  centerSaturation: 1.55,
  centerStrokeOpacity: 1,
  ditherOpacity: 1,
  backdropGradientStrength: 0.75,
  edgeHighlightBrightness: 6.5,
  edgeHighlightSize: 1,
  haloBrightness: 1.45,
  haloFalloff: 1,
  pointerResponseRadius: 1,
  pointerSensingDistance: 1,
  refractionStrength: 1,
  dialogBlur: 1,
  dialogOpacity: 1,
  dialogOverlayStrength: 1,
  dialogBackdropDim: 0.52
};

export const INTERFACE_STYLE_LIMITS = {
  windowCornerRadius: { min: 12, max: 12, step: 1 },
  surfaceCornerRadius: { min: 10, max: 28, step: 1 },
  controlCornerRadius: { min: 6, max: 20, step: 1 },
  contentCornerRadius: { min: 0, max: 18, step: 1 },
  sideBlur: { min: 0, max: 2, step: 0.05 },
  sideOpacity: { min: 0, max: 2, step: 0.05 },
  sideOverlayStrength: { min: 0, max: 2, step: 0.05 },
  sideSaturation: { min: 1, max: 2.2, step: 0.05 },
  sideStrokeOpacity: { min: 0, max: 2, step: 0.05 },
  centerBlur: { min: 0, max: 2, step: 0.05 },
  centerOpacity: { min: 0, max: 2, step: 0.05 },
  centerOverlayStrength: { min: 0, max: 2, step: 0.05 },
  centerSaturation: { min: 1, max: 2.2, step: 0.05 },
  centerStrokeOpacity: { min: 0, max: 2, step: 0.05 },
  ditherOpacity: { min: 0, max: 2, step: 0.05 },
  backdropGradientStrength: { min: 0, max: 1.5, step: 0.05 },
  edgeHighlightBrightness: { min: 0, max: 8, step: 0.25 },
  edgeHighlightSize: { min: 0.4, max: 2, step: 0.05 },
  haloBrightness: { min: 0, max: 2, step: 0.05 },
  haloFalloff: { min: 0.4, max: 2, step: 0.05 },
  pointerResponseRadius: { min: 0.4, max: 2, step: 0.05 },
  pointerSensingDistance: { min: 0.4, max: 2.5, step: 0.05 },
  refractionStrength: { min: 0, max: 2, step: 0.05 },
  dialogBlur: { min: 0, max: 2, step: 0.05 },
  dialogOpacity: { min: 0, max: 2, step: 0.05 },
  dialogOverlayStrength: { min: 0, max: 2, step: 0.05 },
  dialogBackdropDim: { min: 0, max: 0.85, step: 0.01 }
} as const;

export type InterfaceStylePatch = Partial<InterfaceStylePreference>;

export interface InterfaceStyleController {
  value: InterfaceStylePreference;
  limits: typeof INTERFACE_STYLE_LIMITS;
  setInterfaceStyle: (patch: InterfaceStylePatch) => void;
  resetInterfaceStyle: () => void;
}

export function normalizeInterfaceStyle(input: unknown): InterfaceStylePreference {
  const source = isRecord(input) ? input : {};
  return {
    windowCornerRadius: clampNumber(
      legacyValue(source, "windowCornerRadius", "windowRadius"),
      INTERFACE_STYLE_LIMITS.windowCornerRadius,
      DEFAULT_INTERFACE_STYLE.windowCornerRadius
    ),
    surfaceCornerRadius: clampNumber(
      legacyValue(source, "surfaceCornerRadius", "panelRadius"),
      INTERFACE_STYLE_LIMITS.surfaceCornerRadius,
      DEFAULT_INTERFACE_STYLE.surfaceCornerRadius
    ),
    controlCornerRadius: clampNumber(
      legacyValue(source, "controlCornerRadius", "controlRadius"),
      INTERFACE_STYLE_LIMITS.controlCornerRadius,
      DEFAULT_INTERFACE_STYLE.controlCornerRadius
    ),
    contentCornerRadius: clampNumber(
      legacyValue(source, "contentCornerRadius", "contentRadius"),
      INTERFACE_STYLE_LIMITS.contentCornerRadius,
      DEFAULT_INTERFACE_STYLE.contentCornerRadius
    ),
    sideBlur: clampNumber(
      legacyValue(source, "sideBlur", "glassBlur"),
      INTERFACE_STYLE_LIMITS.sideBlur,
      DEFAULT_INTERFACE_STYLE.sideBlur
    ),
    sideOpacity: clampNumber(
      legacyValue(source, "sideOpacity", "glassOpacity"),
      INTERFACE_STYLE_LIMITS.sideOpacity,
      DEFAULT_INTERFACE_STYLE.sideOpacity
    ),
    sideOverlayStrength: clampNumber(
      legacyValue(source, "sideOverlayStrength", "glassOverlayStrength"),
      INTERFACE_STYLE_LIMITS.sideOverlayStrength,
      DEFAULT_INTERFACE_STYLE.sideOverlayStrength
    ),
    sideOverlayColor: normalizeColor(source.sideOverlayColor, DEFAULT_INTERFACE_STYLE.sideOverlayColor),
    sideSaturation: clampNumber(
      legacyValue(source, "sideSaturation", "glassSaturation"),
      INTERFACE_STYLE_LIMITS.sideSaturation,
      DEFAULT_INTERFACE_STYLE.sideSaturation
    ),
    sideStrokeOpacity: clampNumber(
      source.sideStrokeOpacity,
      INTERFACE_STYLE_LIMITS.sideStrokeOpacity,
      DEFAULT_INTERFACE_STYLE.sideStrokeOpacity
    ),
    centerBlur: clampNumber(
      legacyValue(source, "centerBlur", "glassBlur"),
      INTERFACE_STYLE_LIMITS.centerBlur,
      DEFAULT_INTERFACE_STYLE.centerBlur
    ),
    centerOpacity: clampNumber(
      legacyValue(source, "centerOpacity", "glassOpacity"),
      INTERFACE_STYLE_LIMITS.centerOpacity,
      DEFAULT_INTERFACE_STYLE.centerOpacity
    ),
    centerOverlayStrength: clampNumber(
      legacyValue(source, "centerOverlayStrength", "glassOverlayStrength"),
      INTERFACE_STYLE_LIMITS.centerOverlayStrength,
      DEFAULT_INTERFACE_STYLE.centerOverlayStrength
    ),
    centerOverlayColor: normalizeColor(
      source.centerOverlayColor,
      DEFAULT_INTERFACE_STYLE.centerOverlayColor
    ),
    centerSaturation: clampNumber(
      legacyValue(source, "centerSaturation", "glassSaturation"),
      INTERFACE_STYLE_LIMITS.centerSaturation,
      DEFAULT_INTERFACE_STYLE.centerSaturation
    ),
    centerStrokeOpacity: clampNumber(
      source.centerStrokeOpacity,
      INTERFACE_STYLE_LIMITS.centerStrokeOpacity,
      DEFAULT_INTERFACE_STYLE.centerStrokeOpacity
    ),
    ditherOpacity: clampNumber(
      source.ditherOpacity,
      INTERFACE_STYLE_LIMITS.ditherOpacity,
      DEFAULT_INTERFACE_STYLE.ditherOpacity
    ),
    backdropGradientStrength: clampNumber(
      source.backdropGradientStrength,
      INTERFACE_STYLE_LIMITS.backdropGradientStrength,
      DEFAULT_INTERFACE_STYLE.backdropGradientStrength
    ),
    edgeHighlightBrightness: clampNumber(
      source.edgeHighlightBrightness,
      INTERFACE_STYLE_LIMITS.edgeHighlightBrightness,
      DEFAULT_INTERFACE_STYLE.edgeHighlightBrightness
    ),
    edgeHighlightSize: clampNumber(
      source.edgeHighlightSize,
      INTERFACE_STYLE_LIMITS.edgeHighlightSize,
      DEFAULT_INTERFACE_STYLE.edgeHighlightSize
    ),
    haloBrightness: clampNumber(
      legacyValue(source, "haloBrightness", "pointerGlowBrightness"),
      INTERFACE_STYLE_LIMITS.haloBrightness,
      DEFAULT_INTERFACE_STYLE.haloBrightness
    ),
    haloFalloff: clampNumber(
      source.haloFalloff,
      INTERFACE_STYLE_LIMITS.haloFalloff,
      DEFAULT_INTERFACE_STYLE.haloFalloff
    ),
    pointerResponseRadius: clampNumber(
      legacyValue(source, "pointerResponseRadius", "pointerGlowSize"),
      INTERFACE_STYLE_LIMITS.pointerResponseRadius,
      DEFAULT_INTERFACE_STYLE.pointerResponseRadius
    ),
    pointerSensingDistance: clampNumber(
      source.pointerSensingDistance,
      INTERFACE_STYLE_LIMITS.pointerSensingDistance,
      DEFAULT_INTERFACE_STYLE.pointerSensingDistance
    ),
    refractionStrength: clampNumber(
      source.refractionStrength,
      INTERFACE_STYLE_LIMITS.refractionStrength,
      DEFAULT_INTERFACE_STYLE.refractionStrength
    ),
    dialogBlur: clampNumber(
      legacyValue(source, "dialogBlur", "glassBlur"),
      INTERFACE_STYLE_LIMITS.dialogBlur,
      DEFAULT_INTERFACE_STYLE.dialogBlur
    ),
    dialogOpacity: clampNumber(
      legacyValue(source, "dialogOpacity", "glassOpacity"),
      INTERFACE_STYLE_LIMITS.dialogOpacity,
      DEFAULT_INTERFACE_STYLE.dialogOpacity
    ),
    dialogOverlayStrength: clampNumber(
      legacyValue(source, "dialogOverlayStrength", "glassOverlayStrength"),
      INTERFACE_STYLE_LIMITS.dialogOverlayStrength,
      DEFAULT_INTERFACE_STYLE.dialogOverlayStrength
    ),
    dialogBackdropDim: clampNumber(
      source.dialogBackdropDim,
      INTERFACE_STYLE_LIMITS.dialogBackdropDim,
      DEFAULT_INTERFACE_STYLE.dialogBackdropDim
    )
  };
}

export function readInterfaceStyle(storage: Storage | undefined = defaultStorage()) {
  if (!storage) return DEFAULT_INTERFACE_STYLE;
  try {
    const raw = storage.getItem(INTERFACE_STYLE_STORAGE_KEY);
    return raw ? normalizeInterfaceStyle(JSON.parse(raw)) : DEFAULT_INTERFACE_STYLE;
  } catch {
    return DEFAULT_INTERFACE_STYLE;
  }
}

export function writeInterfaceStyle(
  value: InterfaceStylePreference,
  storage: Storage | undefined = defaultStorage()
) {
  if (!storage) return;
  storage.setItem(INTERFACE_STYLE_STORAGE_KEY, JSON.stringify(normalizeInterfaceStyle(value)));
}

export function interfaceStyleToCssVariables(
  value: InterfaceStylePreference
): Record<string, string> {
  const normalized = normalizeInterfaceStyle(value);
  const overlayRadius = clampRounded(
    normalized.surfaceCornerRadius + 4,
    12,
    40
  );
  const insetSurfaceRadius = clampRounded(
    normalized.surfaceCornerRadius - 4,
    6,
    normalized.surfaceCornerRadius
  );
  const tightRadius = clampRounded(
    normalized.controlCornerRadius - 4,
    0,
    normalized.controlCornerRadius
  );
  const sideOverlay = colorWithAlpha(
    normalized.sideOverlayColor,
    0.38 * normalized.sideOverlayStrength * normalized.sideOpacity
  );
  const sideOverlayStrong = colorWithAlpha(
    normalized.sideOverlayColor,
    0.5 * normalized.sideOverlayStrength * normalized.sideOpacity
  );
  const centerOverlay = colorWithAlpha(
    normalized.centerOverlayColor,
    0.38 * normalized.centerOverlayStrength * normalized.centerOpacity
  );
  const centerOverlayStrong = colorWithAlpha(
    normalized.centerOverlayColor,
    0.5 * normalized.centerOverlayStrength * normalized.centerOpacity
  );
  const controlAlpha = 0.085 * normalized.centerOpacity;
  const clearDimAlpha = 0.52 * normalized.centerOverlayStrength;
  const pointerGlowSize = 150 * normalized.pointerResponseRadius;
  const pointerLensSize = 124 * normalized.pointerResponseRadius * normalized.refractionStrength;
  const pointerAuraSize = 176 * normalized.pointerResponseRadius * normalized.haloFalloff;
  const pointerSensingDistance = POINTER_SENSING_DISTANCE_BASE_PX * normalized.pointerSensingDistance;
  const pointerFillOpacity = 0.028 * normalized.haloBrightness;
  const pointerPressOpacity = 0.03 * normalized.haloBrightness;
  const borderHighlightSize = 92 * normalized.edgeHighlightSize;
  const dialogFill = colorWithAlpha(
    normalized.centerOverlayColor,
    0.5 * normalized.dialogOverlayStrength * normalized.dialogOpacity
  );
  const ditherOpacity = 0.055 * normalized.ditherOpacity;
  return {
    "--radius-window": `${roundCssNumber(normalized.windowCornerRadius)}px`,
    "--radius-overlay": `${roundCssNumber(overlayRadius)}px`,
    "--radius-panel": `${roundCssNumber(normalized.surfaceCornerRadius)}px`,
    "--radius-surface": `${roundCssNumber(insetSurfaceRadius)}px`,
    "--radius-control": `${roundCssNumber(normalized.controlCornerRadius)}px`,
    "--radius-content": `${roundCssNumber(normalized.contentCornerRadius)}px`,
    "--radius-tight": `${roundCssNumber(tightRadius)}px`,
    "--glass-side-blur": `${roundCssNumber(26 * normalized.sideBlur)}px`,
    "--glass-side-fill": sideOverlay,
    "--glass-side-fill-strong": sideOverlayStrong,
    "--glass-side-saturation": String(roundCssNumber(normalized.sideSaturation)),
    "--glass-side-stroke": rgbWithAlpha(255, 255, 255, 0.14 * normalized.sideStrokeOpacity),
    "--glass-center-blur": `${roundCssNumber(26 * normalized.centerBlur)}px`,
    "--glass-center-fill": centerOverlay,
    "--glass-center-fill-strong": centerOverlayStrong,
    "--glass-center-saturation": String(roundCssNumber(normalized.centerSaturation)),
    "--glass-center-stroke": rgbWithAlpha(255, 255, 255, 0.14 * normalized.centerStrokeOpacity),
    "--glass-dither-opacity": String(roundCssNumber(ditherOpacity)),
    "--glass-backdrop-gradient-opacity": String(roundCssNumber(normalized.backdropGradientStrength)),
    "--glass-halo-brightness": String(roundCssNumber(normalized.haloBrightness)),
    "--glass-halo-falloff": String(roundCssNumber(normalized.haloFalloff)),
    "--glass-pointer-response-radius": String(roundCssNumber(normalized.pointerResponseRadius)),
    "--glass-refraction-strength": String(roundCssNumber(normalized.refractionStrength)),
    "--glass-edge-highlight-brightness": String(
      roundCssNumber(normalized.edgeHighlightBrightness)
    ),
    "--glass-border-highlight-size": `${roundCssNumber(borderHighlightSize)}px`,
    "--glass-dialog-blur": `${roundCssNumber(34 * normalized.dialogBlur)}px`,
    "--glass-dialog-fill": dialogFill,
    "--dialog-blur": `${roundCssNumber(18 * normalized.dialogBlur)}px`,
    "--dialog-backdrop-dim": rgbWithAlpha(2, 5, 8, normalized.dialogBackdropDim),
    "--glass-blur": `${roundCssNumber(26 * normalized.centerBlur)}px`,
    "--glass-elevated-blur": `${roundCssNumber(34 * normalized.dialogBlur)}px`,
    "--glass-control-blur": `${roundCssNumber(18 * normalized.centerBlur)}px`,
    "--glass-saturation": String(roundCssNumber(normalized.centerSaturation)),
    "--glass-background-glow-brightness": String(roundCssNumber(normalized.haloBrightness)),
    "--glass-control": rgbWithAlpha(246, 252, 255, controlAlpha),
    "--glass-readable-surface": centerOverlay,
    "--glass-readable-surface-strong": centerOverlayStrong,
    "--glass-clear-dim": rgbWithAlpha(2, 5, 8, clearDimAlpha),
    "--glass-pointer-glow-brightness": String(roundCssNumber(normalized.haloBrightness)),
    "--glass-pointer-glow-size": `${roundCssNumber(pointerGlowSize)}px`,
    "--glass-pointer-lens-size": `${roundCssNumber(pointerLensSize)}px`,
    "--glass-pointer-aura-size": `${roundCssNumber(pointerAuraSize)}px`,
    "--glass-pointer-sensing-distance": `${roundCssNumber(pointerSensingDistance)}px`,
    "--glass-pointer-fill-opacity": String(roundCssNumber(pointerFillOpacity)),
    "--glass-pointer-press-opacity": String(roundCssNumber(pointerPressOpacity))
  };
}

export function applyInterfaceStyleVariables(
  value: InterfaceStylePreference,
  target: HTMLElement = document.documentElement
) {
  const variables = interfaceStyleToCssVariables(value);
  for (const [name, cssValue] of Object.entries(variables)) {
    target.style.setProperty(name, cssValue);
  }
}

export function useInterfaceStyle(): InterfaceStyleController {
  const [value, setValue] = useState<InterfaceStylePreference>(() => readInterfaceStyle());

  useEffect(() => {
    applyInterfaceStyleVariables(value);
    writeInterfaceStyle(value);
  }, [value]);

  const setInterfaceStyle = useCallback((patch: InterfaceStylePatch) => {
    setValue((current) => normalizeInterfaceStyle({ ...current, ...patch }));
  }, []);

  const resetInterfaceStyle = useCallback(() => {
    setValue(DEFAULT_INTERFACE_STYLE);
  }, []);

  return useMemo(
    () => ({ value, limits: INTERFACE_STYLE_LIMITS, setInterfaceStyle, resetInterfaceStyle }),
    [resetInterfaceStyle, setInterfaceStyle, value]
  );
}

function defaultStorage(): Storage | undefined {
  return typeof window === "undefined" ? undefined : window.localStorage;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function legacyValue(
  source: Record<string, unknown>,
  preferredKey: string,
  legacyKey: string
) {
  return source[preferredKey] ?? source[legacyKey];
}

function clampNumber(value: unknown, limits: { min: number; max: number }, fallback: number) {
  const number = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.min(limits.max, Math.max(limits.min, number));
}

function normalizeColor(value: unknown, fallback: string) {
  if (typeof value !== "string") {
    return fallback;
  }
  const color = value.trim();
  return /^#[0-9a-fA-F]{6}$/.test(color) ? color.toLowerCase() : fallback;
}

function colorWithAlpha(color: string, alpha: number) {
  const rgb = hexToRgb(color);
  return rgbWithAlpha(rgb.red, rgb.green, rgb.blue, alpha);
}

function hexToRgb(color: string) {
  const normalized = normalizeColor(color, "#000000");
  return {
    red: Number.parseInt(normalized.slice(1, 3), 16),
    green: Number.parseInt(normalized.slice(3, 5), 16),
    blue: Number.parseInt(normalized.slice(5, 7), 16)
  };
}

function clampRounded(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function rgbWithAlpha(red: number, green: number, blue: number, alpha: number) {
  return `rgb(${red} ${green} ${blue} / ${roundCssNumber(Math.min(1, Math.max(0, alpha)))})`;
}

function roundCssNumber(value: number) {
  return Math.round(value * 1000) / 1000;
}
