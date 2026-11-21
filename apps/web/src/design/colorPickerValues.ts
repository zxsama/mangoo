export interface HsvColor {
  hue: number;
  saturation: number;
  brightness: number;
}

export function normalizePickerHex(text: string): string | null {
  const digits = text.trim().replace(/^#/, "");
  if (!/^(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(digits)) return null;
  return "#" + (digits.length === 3 ? [...digits].map(char => char + char).join("") : digits).toLowerCase();
}

export function hexToHsv(hex: string, previousHue = 0): HsvColor {
  const [r, g, b] = [1, 3, 5].map(offset => parseInt(hex.slice(offset, offset + 2), 16) / 255);
  const high = Math.max(r, g, b);
  const delta = high - Math.min(r, g, b);
  let hue = previousHue;
  if (delta > 0) {
    const sector = high === r ? (g - b) / delta : high === g ? (b - r) / delta + 2 : (r - g) / delta + 4;
    hue = (60 * sector + 360) % 360;
  }
  return { hue, saturation: high === 0 ? 0 : delta / high * 100, brightness: high * 100 };
}

export function hsvToHex({ hue, saturation, brightness }: HsvColor): string {
  const v = brightness / 100;
  const c = v * saturation / 100;
  const sector = (hue % 360) / 60;
  const x = c * (1 - Math.abs(sector % 2 - 1));
  const rgb = [[c, x, 0], [x, c, 0], [0, c, x], [0, x, c], [x, 0, c], [c, 0, x]][Math.floor(sector)];
  return "#" + rgb.map(channel => Math.round(255 * (channel + v - c)).toString(16).padStart(2, "0")).join("");
}
