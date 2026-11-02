import { Check } from "../../design/icons";
import { OverlaySelect } from "../../design/overlays";
import { useEffect, useId, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { IDENTITY_ICONS, IDENTITY_CATEGORIES, IdentityArtwork, matchesIdentityIcon, resolveIdentityIcon } from "./identityArtwork";

export const LIBRARY_ICONS = IDENTITY_ICONS;

export const LIBRARY_COLORS: string[] = [
  "#ffffff",
  "#7cc4ff",
  "#5ce1a6",
  "#ffd166",
  "#ff8c69",
  "#f06595",
  "#b197fc",
  "#4dd0e1",
  "#a3e635",
  "#ff922b",
  "#e5e7eb"
];

export const DEFAULT_LIBRARY_ICON = "library";
export const DEFAULT_LIBRARY_COLOR = "#ffffff";

export const resolveLibraryIcon = resolveIdentityIcon;

export function LibraryGlyph({ icon, color, size = 15 }: {
  icon?: string;
  color?: string;
  selected?: boolean;
  size?: number;
}) {
  return <IdentityArtwork icon={icon} color={color || DEFAULT_LIBRARY_COLOR} size={size} />;
}

// Controlled draft choices shared by all Library appearance entrypoints.
export interface LibraryIconPickerProps {
  kind?: "library" | "folder";
  color?: string;
  disabled?: boolean;
  icon?: string;
  searchRef?: RefObject<HTMLInputElement>;
  onNativeColorOpen?: () => void;
  onColorValidityChange?: (valid: boolean) => void;
  onPick: (icon: string, color: string) => void;
}

const COLOR_LABELS = ["白色", "蓝色", "绿色", "黄色", "珊瑚色", "粉色", "紫色", "青色", "黄绿色", "橙色", "灰色"];

/** Presentation only: never normalize or rewrite the saved CSS color. */
export function libraryColorCheckColor(color: string): string {
  let rgb: number[] | undefined;
  const hex = /^#([\da-f]{3}|[\da-f]{6})$/i.exec(color)?.[1];
  if (hex) {
    const expanded = hex.length === 3 ? [...hex].map((part) => part + part).join("") : hex;
    rgb = [0, 2, 4].map((offset) => Number.parseInt(expanded.slice(offset, offset + 2), 16));
  } else if (typeof document !== "undefined") {
    // The contract retains arbitrary CSS color strings. Let the browser parse
    // its supported formats instead of imposing a new hex-only data contract.
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (context) {
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      rgb = [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
    }
  }
  if (!rgb) return "#18202b";
  const linear = rgb.map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  const luminance = linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
  // Relative luminance of #18202b is 0.0141. Choose the greater contrast.
  return 1.05 / (luminance + 0.05) > (luminance + 0.05) / 0.0641 ? "#ffffff" : "#18202b";
}

export function LibraryIconPicker({ icon, color, disabled = false, onPick, onColorValidityChange, onNativeColorOpen, searchRef, kind = "library" }: LibraryIconPickerProps) {
  const currentIcon = icon ?? (kind === "folder" ? "folder" : DEFAULT_LIBRARY_ICON);
  const identityLabel = kind === "folder" ? "文件夹" : "资源库";
  const currentColor = color ?? DEFAULT_LIBRARY_COLOR;
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("all");
  const [colorText, setColorText] = useState(currentColor);
  const [colorEdited, setColorEdited] = useState(false);
  const colorErrorId = useId();
  const colorValid = !colorEdited || /^#([\da-f]{3}|[\da-f]{6})$/i.test(colorText.trim());
  const selectedColorRef = useRef<HTMLSpanElement>(null);
  const [checkColor, setCheckColor] = useState(() => libraryColorCheckColor(currentColor));
  useEffect(() => {
    const swatch = selectedColorRef.current;
    setCheckColor(libraryColorCheckColor(swatch ? getComputedStyle(swatch).backgroundColor : currentColor));
  }, [currentColor]);
  const groups: Array<{ key: string; label: string; icons: ReadonlyArray<{ key: string; label: string }> }> = IDENTITY_CATEGORIES
    .filter((entry) => category === "all" || entry.key === category)
    .map((entry) => ({ ...entry, icons: LIBRARY_ICONS.filter((icon) => icon.category === entry.key && matchesIdentityIcon(icon, query)) }))
    .filter((entry) => entry.icons.length > 0);
  if (!LIBRARY_ICONS.some((entry) => entry.key === currentIcon) && !query && category === "all") {
    groups.push({ key: "current", label: "当前图标", icons: [{ key: currentIcon, label: "当前图标" }] });
  }
  const icons = groups.flatMap((entry) => entry.icons);
  const tabIcon = icons.some((entry) => entry.key === currentIcon) ? currentIcon : icons[0]?.key;
  function selectColor(value: string) {
    setColorText(value);
    setColorEdited(false);
    onColorValidityChange?.(true);
    onPick(currentIcon, value);
  }
  function editColor(value: string) {
    setColorText(value);
    setColorEdited(true);
    const valid = /^#([\da-f]{3}|[\da-f]{6})$/i.test(value.trim());
    onColorValidityChange?.(valid);
    if (valid) onPick(currentIcon, value.trim());
  }
  const nativeColor = /^#[\da-f]{6}$/i.test(currentColor) ? currentColor
    : /^#[\da-f]{3}$/i.test(currentColor) ? `#${[...currentColor.slice(1)].map((part) => part + part).join("")}` : DEFAULT_LIBRARY_COLOR;
  const tabColor = LIBRARY_COLORS.includes(currentColor) ? currentColor : null;

  function navigate(event: KeyboardEvent<HTMLDivElement>) {
    if (disabled || event.altKey || event.ctrlKey || event.metaKey) return;
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button[role="radio"]:not(:disabled)')];
    const current = buttons.findIndex((button) => button === event.target);
    if (current < 0 || buttons.length === 0) return;
    event.preventDefault();
    event.stopPropagation();
    let next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
      : (current + (event.key === "ArrowLeft" ? -1 : 1) + buttons.length) % buttons.length;
    if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      // Group boundaries and partial rows cannot be navigated with a fixed column stride.
      const rows: Array<Array<{ index: number; top: number; center: number }>> = [];
      buttons.forEach((button, index) => {
        const rect = button.getBoundingClientRect();
        const cell = { index, top: rect.top, center: rect.left + rect.width / 2 };
        const row = rows[rows.length - 1];
        // Shared pointer feedback can lift a hovered cell by one pixel. Treat
        // vertically overlapping cells as one row, including at enlarged zoom.
        if (row && Math.abs(row[0].top - rect.top) < rect.height / 2) row.push(cell);
        else rows.push([cell]);
      });
      const rowIndex = rows.findIndex((row) => row.some((cell) => cell.index === current));
      const center = rows[rowIndex].find((cell) => cell.index === current)!.center;
      const targetRow = rows[(rowIndex + (event.key === "ArrowUp" ? -1 : 1) + rows.length) % rows.length];
      next = targetRow.reduce((nearest, cell) => Math.abs(cell.center - center) < Math.abs(nearest.center - center) ? cell : nearest).index;
    }
    buttons[next].focus();
    buttons[next].click();
  }

  return <div className="library-icon-picker">
    <div className="library-icon-picker-tools">
      <input aria-label="搜索图标" className="library-icon-search" disabled={disabled} onChange={(event) => setQuery(event.target.value)} placeholder="搜索图标" ref={searchRef} type="search" value={query} />
      <OverlaySelect aria-label="图标分类" className="library-icon-category" disabled={disabled} label="图标分类" onChange={(event) => setCategory(event.target.value)} value={category}>
        <option value="all">全部图标</option>
        {IDENTITY_CATEGORIES.map((entry) => <option key={entry.key} value={entry.key}>{entry.label}</option>)}
      </OverlaySelect>
    </div>
    <div aria-label={`${identityLabel}图标`} className="library-icon-picker-grid" onKeyDown={navigate} role="radiogroup">
      {groups.map((group) => <div className="library-icon-picker-group" data-icon-category={group.key} key={group.key}>
        <h3 className="library-icon-picker-group-heading">{group.label}</h3>
        <div className="library-icon-picker-group-grid">
          {group.icons.map(({ key, label }) => <button
            aria-checked={key === currentIcon} aria-label={label} className="library-icon-picker-cell"
            data-icon={key} data-interactive-pointer-target="true" disabled={disabled} key={key}
            onClick={() => onPick(key, currentColor)} role="radio" tabIndex={key === tabIcon ? 0 : -1} title={label} type="button"
          ><IdentityArtwork icon={key} color="currentColor" variant="symbol" size={18} /></button>)}
        </div>
      </div>)}
      {icons.length === 0 ? <span className="library-icon-picker-empty" role="status">未找到图标</span> : null}
    </div>
    <div aria-label={`${identityLabel}颜色`} className="library-icon-picker-colors" onKeyDown={navigate} role="radiogroup">
      <span className="library-appearance-group-label">颜色</span>
      {[...LIBRARY_COLORS, ...(tabColor === null ? [currentColor] : [])].map((value, index) => <button
        aria-checked={value === currentColor} aria-label={`${COLOR_LABELS[index] ?? "自定义颜色"} ${value}`} className="library-icon-picker-swatch"
        data-color={value} data-interactive-pointer-target="true" disabled={disabled} key={value}
        onClick={() => selectColor(value)} role="radio" tabIndex={value === currentColor ? 0 : -1} title={`${COLOR_LABELS[index] ?? "自定义颜色"} ${value}`} type="button"
      ><span className="library-icon-picker-color" ref={value === currentColor ? selectedColorRef : undefined} style={{ backgroundColor: value }} />{value === currentColor ? <Check aria-hidden="true" className="library-icon-picker-check" size={14} style={{ color: checkColor }} /> : null}</button>)}
    </div>
    <div className="library-custom-color">
      <label className="library-custom-color-native">自定义
        <input aria-label="自定义颜色" disabled={disabled} onClick={onNativeColorOpen} onChange={(event) => selectColor(event.target.value)} type="color" value={nativeColor} />
      </label>
      <input aria-describedby={!colorValid ? colorErrorId : undefined} aria-invalid={!colorValid || undefined} aria-label="HEX 颜色" className="library-custom-color-hex" disabled={disabled} onChange={(event) => editColor(event.target.value)} placeholder="#RRGGBB" spellCheck={false} type="text" value={colorText} />
      {!colorValid ? <span className="library-custom-color-error" id={colorErrorId} role="alert">请输入 #RGB 或 #RRGGBB 颜色</span> : null}
    </div>
  </div>;
}
