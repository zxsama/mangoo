export type LibraryLayoutMode = "adaptive" | "waterfall" | "grid" | "list";

export const LIBRARY_LAYOUT_MODES: Array<{
  value: LibraryLayoutMode;
  label: string;
}> = [
  { value: "adaptive", label: "Adaptive" },
  { value: "waterfall", label: "Waterfall" },
  { value: "grid", label: "Grid" },
  { value: "list", label: "List" }
];

export const DEFAULT_LIBRARY_LAYOUT_MODE: LibraryLayoutMode = "adaptive";

export function isLibraryLayoutMode(value: string | null | undefined): value is LibraryLayoutMode {
  return value === "adaptive" || value === "waterfall" || value === "grid" || value === "list";
}
