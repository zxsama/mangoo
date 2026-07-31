import { Heart, SlidersHorizontal, Star, X } from "lucide-react";
import { useRef } from "react";
import type { TagRecord } from "@megle/core-client";
import {
  AnchoredOverlay,
  OverlayMenuItem,
  OverlayMenuList
} from "../../design/overlays";
import { LiquidGlassButton } from "../../design/liquid-glass";

type Kind = "image" | "video" | "other";
type MinRating = 1 | 2 | 3 | 4 | 5;

interface FilterMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  kind?: Kind;
  minRating?: MinRating;
  favorite?: boolean;
  tagIds: number[];
  tags: TagRecord[];
  onSetKind: (kind: Kind | undefined) => void;
  onSetMinRating: (rating: MinRating | undefined) => void;
  onToggleFavorite: () => void;
  onToggleTag: (tagId: number) => void;
  onClear: () => void;
}

const KINDS: Array<{ value: Kind; label: string }> = [
  { value: "image", label: "Images" },
  { value: "video", label: "Videos" },
  { value: "other", label: "Other" }
];

const RATINGS: MinRating[] = [1, 2, 3, 4, 5];

export function FilterMenu({
  favorite,
  kind,
  minRating,
  onClear,
  onOpenChange,
  onSetKind,
  onSetMinRating,
  onToggleFavorite,
  onToggleTag,
  open,
  tagIds,
  tags
}: FilterMenuProps) {
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const hasFilters =
    kind !== undefined ||
    minRating !== undefined ||
    favorite !== undefined ||
    tagIds.length > 0;

  return (
    <div className="filter-menu">
      <LiquidGlassButton
        ref={buttonRef}
        active={open || hasFilters}
        aria-controls="filter-root"
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label="Filters"
        className="filter-menu-trigger"
        data-titlebar-control="library-filter"
        onClick={() => onOpenChange(!open)}
        title="Filters"
        tone="control"
        type="button"
      >
        <SlidersHorizontal aria-hidden="true" size={15} />
        {hasFilters ? <span className="filter-menu-dot" aria-hidden="true" /> : null}
      </LiquidGlassButton>

      <AnchoredOverlay
        anchor={buttonRef.current}
        ariaLabel="Filters"
        className="overlay-titlebar-popover filter-menu-root"
        id="filter-root"
        onDismiss={() => onOpenChange(false)}
        open={open}
        placement="bottom-start"
        preferredWidth={320}
        restoreFocusTarget={buttonRef.current}
        role="group"
        rootId="filter"
      >
        <OverlayMenuList ariaLabel="Filters" className="filter-menu-content">
          <div className="filter-menu-section" aria-label="Kind" role="group">
            <div className="filter-menu-section-title">Kind</div>
            <div className="filter-menu-options">
              {KINDS.map((option) => (
                <button
                  aria-checked={kind === option.value}
                  className="filter-menu-item"
                  data-overlay-menu-item="true"
                  key={option.value}
                  onClick={() => onSetKind(kind === option.value ? undefined : option.value)}
                  role="menuitemcheckbox"
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>

          <div className="filter-menu-section" aria-label="Rating" role="group">
            <div className="filter-menu-section-title">Rating</div>
            <div className="filter-menu-options filter-menu-rating">
              {RATINGS.map((rating) => (
                <button
                  aria-checked={minRating === rating}
                  aria-label={`Minimum ${rating} star${rating > 1 ? "s" : ""}`}
                  className="filter-menu-item"
                  data-overlay-menu-item="true"
                  key={rating}
                  onClick={() => onSetMinRating(minRating === rating ? undefined : rating)}
                  role="menuitemcheckbox"
                  type="button"
                >
                  <Star aria-hidden="true" size={12} />
                  <span>{rating}+</span>
                </button>
              ))}
            </div>
          </div>

          <div className="filter-menu-section" aria-label="Favorite" role="group">
            <button
              aria-checked={favorite === true}
              className="filter-menu-item"
              data-overlay-menu-item="true"
              onClick={onToggleFavorite}
              role="menuitemcheckbox"
              type="button"
            >
              <Heart aria-hidden="true" size={13} />
              <span>Favorites</span>
            </button>
          </div>

          <div className="filter-menu-section" aria-label="Tags" role="group">
            <div className="filter-menu-section-title">Tags</div>
            <div className="filter-menu-tags">
              {tags.length > 0 ? (
                tags.map((tag) => (
                  <button
                    aria-checked={tagIds.includes(tag.id)}
                    className="filter-menu-item filter-menu-tag"
                    data-overlay-menu-item="true"
                    key={tag.id}
                    onClick={() => onToggleTag(tag.id)}
                    role="menuitemcheckbox"
                    type="button"
                  >
                    {tag.color ? (
                      <span
                        aria-hidden="true"
                        className="filter-menu-tag-swatch"
                        style={{ background: tag.color }}
                      />
                    ) : null}
                    <span>{tag.name}</span>
                  </button>
                ))
              ) : (
                <span className="filter-menu-empty">No tags</span>
              )}
            </div>
          </div>

          <OverlayMenuItem
            className="filter-menu-clear"
            disabled={!hasFilters}
            icon={<X aria-hidden="true" size={13} />}
            onClick={onClear}
          >
            Clear filters
          </OverlayMenuItem>
        </OverlayMenuList>
      </AnchoredOverlay>
    </div>
  );
}
