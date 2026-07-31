import { ArrowDownUp } from "lucide-react";
import { useRef } from "react";
import {
  AnchoredOverlay,
  OverlayMenuItem,
  OverlayMenuList
} from "../../design/overlays";
import { LiquidGlassButton } from "../../design/liquid-glass";

export type SortOption =
  | "mtime_desc"
  | "mtime_asc"
  | "name_asc"
  | "name_desc"
  | "rating_desc"
  | "rating_asc";

interface SortMenuProps {
  iconOnly?: boolean;
  open: boolean;
  value: SortOption;
  onChange: (sort: SortOption) => void;
  onOpenChange: (open: boolean) => void;
}

const SORT_OPTIONS: Array<{ value: SortOption; label: string }> = [
  { value: "mtime_desc", label: "Newest first" },
  { value: "mtime_asc", label: "Oldest first" },
  { value: "name_asc", label: "Name A–Z" },
  { value: "name_desc", label: "Name Z–A" },
  { value: "rating_desc", label: "Highest rated" },
  { value: "rating_asc", label: "Lowest rated" }
];

const SORT_LABEL: Record<SortOption, string> = Object.fromEntries(
  SORT_OPTIONS.map((option) => [option.value, option.label])
) as Record<SortOption, string>;

export function SortMenu({
  iconOnly = false,
  open,
  value,
  onChange,
  onOpenChange
}: SortMenuProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const selectedLabel = SORT_LABEL[value];

  function handleSelect(sort: SortOption) {
    onChange(sort);
    onOpenChange(false);
  }

  return (
    <div className="sort-menu">
      <LiquidGlassButton
        ref={buttonRef}
        active={open}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={`Sort: ${selectedLabel}`}
        className={`sort-menu-trigger${iconOnly ? " sort-menu-trigger-icon-only" : ""}${
          open ? " sort-menu-trigger-open" : ""
        }`}
        data-titlebar-control="library-sort"
        onClick={() => onOpenChange(!open)}
        title={`Sort: ${selectedLabel}`}
        tone="control"
        type="button"
      >
        <ArrowDownUp aria-hidden="true" size={14} />
        {iconOnly ? null : <span>{selectedLabel}</span>}
      </LiquidGlassButton>

      <AnchoredOverlay
        anchor={buttonRef.current}
        ariaLabel="Sort options"
        className="overlay-titlebar-popover sort-menu-root"
        id="sort-root"
        onDismiss={() => onOpenChange(false)}
        open={open}
        placement="bottom-start"
        preferredWidth={180}
        restoreFocusTarget={buttonRef.current}
        role="listbox"
        rootId="sort"
      >
        <OverlayMenuList
          ariaLabel="Sort options"
          className="overlay-titlebar-menu sort-menu-options"
          role="listbox"
        >
          {SORT_OPTIONS.map((option) => {
            const selected = value === option.value;
            return (
              <OverlayMenuItem
                aria-selected={selected}
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
