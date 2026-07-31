import {
  GalleryVerticalEnd,
  LayoutGrid,
  LayoutList,
  LayoutPanelTop,
  type LucideIcon
} from "lucide-react";
import { useRef } from "react";
import {
  AnchoredOverlay,
  OverlayMenuItem,
  OverlayMenuList
} from "../../design/overlays";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  LIBRARY_LAYOUT_MODES,
  type LibraryLayoutMode
} from "../media-grid/layoutMode";

interface LayoutMenuProps {
  iconOnly?: boolean;
  open: boolean;
  value: LibraryLayoutMode;
  onChange: (mode: LibraryLayoutMode) => void;
  onOpenChange: (open: boolean) => void;
  titlebarControlId?: string;
}

const LAYOUT_ICON: Record<LibraryLayoutMode, LucideIcon> = {
  adaptive: LayoutPanelTop,
  waterfall: GalleryVerticalEnd,
  grid: LayoutGrid,
  list: LayoutList
};

const LAYOUT_LABEL: Record<LibraryLayoutMode, string> = Object.fromEntries(
  LIBRARY_LAYOUT_MODES.map((option) => [option.value, option.label])
) as Record<LibraryLayoutMode, string>;

export function LayoutMenu({
  iconOnly = true,
  open,
  onChange,
  onOpenChange,
  titlebarControlId = "library-layout",
  value
}: LayoutMenuProps) {
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const selectedLabel = LAYOUT_LABEL[value];
  const TriggerIcon = LAYOUT_ICON[value];

  function handleSelect(mode: LibraryLayoutMode) {
    onChange(mode);
    onOpenChange(false);
  }

  return (
    <div className="sort-menu layout-menu">
      <LiquidGlassButton
        ref={buttonRef}
        active={open}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={`Layout: ${selectedLabel}`}
        className={`sort-menu-trigger layout-menu-trigger${
          iconOnly ? " sort-menu-trigger-icon-only" : ""
        }${open ? " sort-menu-trigger-open" : ""}`}
        data-titlebar-control={titlebarControlId}
        onClick={() => onOpenChange(!open)}
        title={`Layout: ${selectedLabel}`}
        tone="control"
        type="button"
      >
        <TriggerIcon aria-hidden="true" size={14} />
        {iconOnly ? null : <span>{selectedLabel}</span>}
      </LiquidGlassButton>

      <AnchoredOverlay
        anchor={buttonRef.current}
        ariaLabel="Layout options"
        className="overlay-titlebar-popover layout-menu-root"
        id="layout-root"
        onDismiss={() => onOpenChange(false)}
        open={open}
        placement="bottom-start"
        preferredWidth={180}
        restoreFocusTarget={buttonRef.current}
        role="listbox"
        rootId="layout"
      >
        <OverlayMenuList
          ariaLabel="Layout options"
          className="overlay-titlebar-menu layout-menu-options"
          role="listbox"
        >
          {LIBRARY_LAYOUT_MODES.map((option) => {
            const OptionIcon = LAYOUT_ICON[option.value];
            const selected = value === option.value;
            return (
              <OverlayMenuItem
                aria-selected={selected}
                icon={<OptionIcon aria-hidden="true" size={14} />}
                key={option.value}
                onClick={() => handleSelect(option.value)}
                role="option"
                selected={selected}
              >
                {option.label}
              </OverlayMenuItem>
            );
          })}
        </OverlayMenuList>
      </AnchoredOverlay>
    </div>
  );
}
