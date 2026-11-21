export const pluginColorTokens = [
  "--accent-fill", "--accent-primary", "--on-accent",
  "--status-success", "--status-warning", "--status-danger", "--status-info",
  "--item-favorite", "--item-rating"
] as const;

export interface PluginUiTheme {
  colorScheme: "light" | "dark";
  colors?: Partial<Record<typeof pluginColorTokens[number], string>>;
}

export function isThemeColor(value: unknown): value is string {
  return typeof value === "string" && /^#[0-9a-f]{6}$/iu.test(value);
}

export function readPluginTheme(value: unknown): PluginUiTheme | undefined {
  if (!value || typeof value !== "object" || !("colorScheme" in value)) return;
  const { colorScheme } = value;
  if (colorScheme !== "light" && colorScheme !== "dark") return;
  if (!("colors" in value)) return { colorScheme };
  if (!value.colors || typeof value.colors !== "object" || Array.isArray(value.colors)) return;
  const input = value.colors as Record<string, unknown>;
  const colors: NonNullable<PluginUiTheme["colors"]> = {};
  for (const token of pluginColorTokens) {
    if (!(token in input)) continue;
    const color = input[token];
    if (!isThemeColor(color)) return;
    colors[token] = color;
  }
  return { colorScheme, colors };
}

// Both the SDK and the Host's compatibility listener set only these agreed
// theme properties. Content colors declared on plugin elements still win.
export function pluginThemeProperties(theme: PluginUiTheme): Record<string, string> {
  const properties: Record<string, string> = { ...theme.colors };
  const accent = theme.colors?.["--accent-primary"];
  if (accent) {
    properties["--accent"] = accent;
    properties["--accent-rgb"] = [1, 3, 5].map(start => parseInt(accent.slice(start, start + 2), 16)).join(" ");
    properties["--accent-muted"] = "rgb(var(--accent-rgb) / 0.16)";
    properties["--interactive-hover"] = properties["--accent-muted"];
    properties["--interactive-active"] = "rgb(var(--accent-rgb) / 0.24)";
    properties["--interactive-focus"] = "rgb(var(--accent-rgb) / 0.72)";
  }
  // Old design.css bundles use these names directly.
  if (theme.colors?.["--status-success"]) properties["--accent-green"] = "var(--status-success)";
  if (theme.colors?.["--status-warning"]) properties["--accent-warn"] = "var(--status-warning)";
  if (theme.colors?.["--status-danger"]) properties["--accent-coral"] = "var(--status-danger)";
  return properties;
}
