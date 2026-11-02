import { X } from "../../design/icons";
import type { TagRecord } from "@mangoo/core-client";

interface TagChipProps {
  tag: TagRecord;
  onRemove?: () => void;
  onClick?: () => void;
  active?: boolean;
  size?: "sm" | "md";
}

export function TagChip({ tag, onRemove, onClick, active = false, size = "md" }: TagChipProps) {
  const className = [
    "tag-chip",
    size === "sm" ? "tag-chip-sm" : "",
    active ? "tag-chip-active" : "",
    onClick ? "tag-chip-clickable" : ""
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <span className={className}>
      {tag.color ? (
        <span
          className="tag-chip-swatch"
          style={{ background: tag.color }}
          aria-hidden="true"
        />
      ) : null}
      {onClick ? (
        <button
          className="tag-chip-label"
          onClick={onClick}
          type="button"
          aria-pressed={active}
        >
          {tag.name}
        </button>
      ) : (
        <span className="tag-chip-label">{tag.name}</span>
      )}
      {onRemove ? (
        <button
          className="tag-chip-remove"
          onClick={onRemove}
          type="button"
          aria-label={`Remove tag ${tag.name}`}
        >
          <X size={10} />
        </button>
      ) : null}
    </span>
  );
}
