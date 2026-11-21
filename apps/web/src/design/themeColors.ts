// sRGB relative luminance, shared by interface accents and user-defined swatches.
function linearChannel(byte: number) {
  const srgb = byte / 255;
  return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
}

export function relativeLuminance(rgb: readonly number[]) {
  return 0.2126 * linearChannel(rgb[0]) + 0.7152 * linearChannel(rgb[1]) + 0.0722 * linearChannel(rgb[2]);
}

export function themeAccentColors(fill: string) {
  const bytes = [1, 3, 5].map(start => Number.parseInt(fill.slice(start, start + 2), 16));
  const luminance = relativeLuminance(bytes);
  // Lift very dark accents for small labels and icons on the dark workbench.
  // Mixing in linear light keeps the resulting luminance continuous across the threshold.
  const white = luminance < 0.32 ? (0.32 - luminance) / (1 - luminance) : 0;
  const channels = white === 0 ? bytes : bytes.map(byte => {
    const linear = linearChannel(byte) * (1 - white) + white;
    return Math.round(255 * (1.055 * linear ** (1 / 2.4) - 0.055));
  });
  return {
    fill,
    foreground: "#" + channels.map(byte => byte.toString(16).padStart(2, "0")).join(""),
    channels: channels.join(" "),
    onFill: 1.05 / (luminance + 0.05) > (luminance + 0.05) / 0.05 ? "#ffffff" : "#000000"
  };
}
