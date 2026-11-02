import { Heart, Star, X } from "../../design/icons";
import type { TagRecord } from "@mangoo/core-client";

type Kind = "image" | "video" | "other";
type MinRating = 1 | 2 | 3 | 4 | 5;

interface FilterChipsProps {
  kind?: Kind;
  minRating?: MinRating;
  favorite?: boolean;
  tagIds: number[];
  tagsById: Map<number, TagRecord>;
  onSetKind: (kind: Kind | undefined) => void;
  onSetMinRating: (rating: MinRating | undefined) => void;
  onToggleFavorite: () => void;
  onToggleTag: (tagId: number) => void;
  onClear: () => void;
}

const KIND_LABELS: Record<Kind, string> = {
  image: "Images",
  video: "Videos",
  other: "Other"
};

const KINDS: Kind[] = ["image", "video", "other"];

export function FilterChips({
  kind,
  minRating,
  favorite,
  tagIds,
  tagsById,
  onSetKind,
  onSetMinRating,
  onToggleFavorite,
  onToggleTag,
  onClear
}: FilterChipsProps) {
  const hasFilters =
    kind !== undefined ||
    minRating !== undefined ||
    favorite !== undefined ||
    tagIds.length > 0;

  return (
    <div className="filter-chips" role="group" aria-label="Active filters">
      {/* Kind filter */}
      <div className="filter-chips-group">
        {KINDS.map((k) => (
          <button
            aria-pressed={kind === k}
            className={`filter-chip${kind === k ? " filter-chip-active" : ""}`}
            key={k}
            onClick={() => onSetKind(kind === k ? undefined : k)}
            type="button"
          >
            {KIND_LABELS[k]}
          </button>
        ))}
      </div>

      {/* Rating filter */}
      <div className="filter-chips-group">
        {([1, 2, 3, 4, 5] as MinRating[]).map((r) => (
          <button
            aria-pressed={minRating === r}
            aria-label={`Minimum ${r} star${r > 1 ? "s" : ""}`}
            className={`filter-chip filter-chip-rating${minRating === r ? " filter-chip-active" : ""}`}
            key={r}
            onClick={() => onSetMinRating(minRating === r ? undefined : r)}
            type="button"
          >
            <Star aria-hidden="true" size={12} weight={minRating === r ? "fill" : "regular"} />
            <span>{r}+</span>
          </button>
        ))}
      </div>

      {/* Favorite filter */}
      <button
        aria-pressed={favorite === true}
        className={`filter-chip filter-chip-favorite${favorite === true ? " filter-chip-active" : ""}`}
        onClick={onToggleFavorite}
        type="button"
      >
        <Heart aria-hidden="true" size={13} weight={favorite === true ? "fill" : "regular"} />
        <span>Favorites</span>
      </button>

      {/* Active tag chips */}
      {tagIds.map((tagId) => {
        const tag = tagsById.get(tagId);
        if (!tag) return null;
        return (
          <button
            aria-pressed={true}
            className="filter-chip filter-chip-active filter-chip-tag"
            key={tagId}
            onClick={() => onToggleTag(tagId)}
            type="button"
          >
            {tag.color ? (
              <span
                className="filter-chip-tag-swatch"
                style={{ background: tag.color }}
                aria-hidden="true"
              />
            ) : null}
            <span>{tag.name}</span>
            <X aria-hidden="true" size={11} />
          </button>
        );
      })}

      {/* Clear all */}
      {hasFilters ? (
        <button
          className="filter-chip-clear"
          onClick={onClear}
          type="button"
          aria-label="Clear all filters"
        >
          <X size={12} />
          <span>Clear</span>
        </button>
      ) : null}
    </div>
  );
}
