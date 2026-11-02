import { Heart, Star } from "../../design/icons";
import type { KeyboardEvent } from "react";
import { useEffect, useId, useMemo, useState } from "react";
import type { TagRecord, UserMetadataRecord } from "@mangoo/core-client";
import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";
import { TagChip } from "../library/TagChip";

interface InspectorMetadataProps {
  fileId: number;
  metadata: UserMetadataRecord | null;
  tags: TagRecord[];
  tagsById: Map<number, TagRecord>;
  saving: boolean;
  onUpdate: (
    fileId: number,
    patch: { rating?: number | null; favorite?: boolean; note?: string | null }
  ) => Promise<void> | void;
  onAddTag: (fileId: number, tagId: number) => Promise<void> | void;
  onRemoveTag: (fileId: number, tagId: number) => Promise<void> | void;
  onCreateTag: (name: string, color?: string | null) => Promise<TagRecord | null>;
}

const STARS: ReadonlyArray<1 | 2 | 3 | 4 | 5> = [1, 2, 3, 4, 5];
const NOTE_SOFT_LIMIT = 500;

export function InspectorMetadata({
  fileId,
  metadata,
  tags,
  tagsById,
  saving,
  onUpdate,
  onAddTag,
  onRemoveTag,
  onCreateTag
}: InspectorMetadataProps) {
  const [noteDraft, setNoteDraft] = useState<string>("");
  const [tagInput, setTagInput] = useState<string>("");
  const [activeSuggestionIndex, setActiveSuggestionIndex] = useState<number>(-1);
  const tagIds = metadata?.tagIds ?? [];
  const suggestionsListId = useId();
  const suggestionOptionId = (index: number) => `${suggestionsListId}-option-${index}`;

  useEffect(() => {
    setNoteDraft(metadata?.note ?? "");
  }, [metadata?.fileId, metadata?.note]);

  useEffect(() => {
    setTagInput("");
    setActiveSuggestionIndex(-1);
  }, [fileId]);

  const rating = metadata?.rating ?? null;
  const favorite = metadata?.favorite === true;

  const matchingTags = useMemo(() => {
    const query = tagInput.trim().toLowerCase();
    if (!query) return [];
    return tags
      .filter((tag) => tag.name.toLowerCase().includes(query) && !tagIds.includes(tag.id))
      .slice(0, 8);
  }, [tagInput, tags, tagIds]);

  const exactMatch = useMemo(
    () => tags.find((tag) => tag.name.toLowerCase() === tagInput.trim().toLowerCase()) ?? null,
    [tags, tagInput]
  );

  // Reset highlight whenever the suggestion set changes.
  useEffect(() => {
    setActiveSuggestionIndex((current) => {
      if (matchingTags.length === 0) return -1;
      if (current < 0 || current >= matchingTags.length) return -1;
      return current;
    });
  }, [matchingTags]);

  function commitNote() {
    const next = noteDraft.length > 2048 ? noteDraft.slice(0, 2048) : noteDraft;
    if ((metadata?.note ?? "") === next) return;
    void onUpdate(fileId, { note: next.length === 0 ? null : next });
  }

  function handleStarClick(value: 1 | 2 | 3 | 4 | 5) {
    if (rating === value) {
      void onUpdate(fileId, { rating: null });
    } else {
      void onUpdate(fileId, { rating: value });
    }
  }

  function attachSuggestion(tag: TagRecord) {
    if (!tagIds.includes(tag.id)) {
      void onAddTag(fileId, tag.id);
    }
    setTagInput("");
    setActiveSuggestionIndex(-1);
  }

  function handleTagSubmit() {
    const name = tagInput.trim();
    if (!name) return;
    if (exactMatch) {
      if (!tagIds.includes(exactMatch.id)) {
        void onAddTag(fileId, exactMatch.id);
      }
      setTagInput("");
      setActiveSuggestionIndex(-1);
      return;
    }
    void (async () => {
      const created = await onCreateTag(name, null);
      if (created) {
        await onAddTag(fileId, created.id);
        setTagInput("");
        setActiveSuggestionIndex(-1);
      }
    })();
  }

  function handleTagInputKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" && matchingTags.length > 0) {
      event.preventDefault();
      setActiveSuggestionIndex((current) => {
        if (current < 0) return 0;
        return (current + 1) % matchingTags.length;
      });
      return;
    }
    if (event.key === "ArrowUp" && matchingTags.length > 0) {
      event.preventDefault();
      setActiveSuggestionIndex((current) => {
        if (current < 0) return matchingTags.length - 1;
        return (current - 1 + matchingTags.length) % matchingTags.length;
      });
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (activeSuggestionIndex >= 0 && activeSuggestionIndex < matchingTags.length) {
        attachSuggestion(matchingTags[activeSuggestionIndex]);
      } else {
        handleTagSubmit();
      }
      return;
    }
    if (event.key === "Escape") {
      if (tagInput.length > 0 || matchingTags.length > 0 || activeSuggestionIndex >= 0) {
        event.preventDefault();
        setTagInput("");
        setActiveSuggestionIndex(-1);
      }
    }
  }

  const showSuggestions = matchingTags.length > 0;
  const activeOptionId =
    showSuggestions && activeSuggestionIndex >= 0
      ? suggestionOptionId(activeSuggestionIndex)
      : undefined;

  return (
    <section className="inspector-metadata" aria-label="Metadata editor">
      <div className="inspector-metadata-row">
        <div className="inspector-label">Rating</div>
        <div
          className="rating-stars"
          role="radiogroup"
          aria-label="Rating"
          aria-busy={saving || undefined}
        >
          {STARS.map((value) => {
            const active = rating !== null && value <= rating;
            return (
              <button
                aria-checked={rating === value}
                aria-label={`${value} star${value > 1 ? "s" : ""}${rating === value ? " (clear)" : ""}`}
                className={`rating-star${active ? " rating-star-active" : ""}`}
                key={value}
                onClick={() => handleStarClick(value)}
                role="radio"
                type="button"
              >
                <Star size={14} weight={active ? "fill" : "regular"} />
              </button>
            );
          })}
          {rating !== null ? (
            <button
              className="rating-clear"
              onClick={() => void onUpdate(fileId, { rating: null })}
              type="button"
            >
              clear
            </button>
          ) : null}
        </div>
      </div>

      <div className="inspector-metadata-row">
        <div className="inspector-label">Favorite</div>
        <button
          aria-pressed={favorite}
          className={`favorite-toggle${favorite ? " favorite-toggle-active" : ""}`}
          onClick={() => void onUpdate(fileId, { favorite: !favorite })}
          type="button"
        >
          <Heart size={14} weight={favorite ? "fill" : "regular"} />
          <span>{favorite ? "Favorited" : "Favorite"}</span>
        </button>
      </div>

      <div className="inspector-metadata-row inspector-metadata-row-tags">
        <div className="inspector-label">Tags</div>
        <div className="inspector-tag-stack">
          <div className="inspector-tag-chips">
            {tagIds.length === 0 ? <span className="empty-hint">No tags</span> : null}
            {tagIds.map((tagId) => {
              const tag = tagsById.get(tagId);
              if (!tag) return null;
              return (
                <TagChip
                  key={tagId}
                  tag={tag}
                  size="sm"
                  onRemove={() => void onRemoveTag(fileId, tagId)}
                />
              );
            })}
          </div>
          <div className="inspector-tag-input-row">
            <input
              aria-activedescendant={activeOptionId}
              aria-autocomplete="list"
              aria-controls={showSuggestions ? suggestionsListId : undefined}
              aria-expanded={showSuggestions}
              aria-label="Add tag"
              autoComplete="off"
              className="inspector-tag-input"
              onChange={(e) => {
                setTagInput(e.target.value);
                setActiveSuggestionIndex(-1);
              }}
              onKeyDown={handleTagInputKeyDown}
              placeholder={exactMatch ? "Add tag" : "Add or create tag"}
              role="combobox"
              type="text"
              value={tagInput}
            />
            <LiquidGlassButton
              disabled={tagInput.trim().length === 0}
              onClick={handleTagSubmit}
              tone="primary"
              type="button"
            >
              {exactMatch ? "Add" : "Create"}
            </LiquidGlassButton>
          </div>
          {showSuggestions ? (
            <LiquidGlassSurface
              as="div"
              aria-label="Tag suggestions"
              className="inspector-tag-suggestions"
              id={suggestionsListId}
              interactive
              role="listbox"
              tone="elevated"
            >
              {matchingTags.map((tag, index) => {
                const isActive = index === activeSuggestionIndex;
                return (
                  <button
                    aria-selected={isActive}
                    className={`inspector-tag-suggestion${isActive ? " inspector-tag-suggestion-active" : ""}`}
                    id={suggestionOptionId(index)}
                    key={tag.id}
                    onClick={() => attachSuggestion(tag)}
                    onMouseEnter={() => setActiveSuggestionIndex(index)}
                    role="option"
                    tabIndex={-1}
                    type="button"
                  >
                    {tag.color ? (
                      <span
                        aria-hidden="true"
                        className="tag-chip-swatch"
                        style={{ background: tag.color }}
                      />
                    ) : null}
                    {tag.name}
                  </button>
                );
              })}
            </LiquidGlassSurface>
          ) : null}
        </div>
      </div>

      <div className="inspector-metadata-row inspector-metadata-row-note">
        <div className="inspector-label">Note</div>
        <div className="inspector-note-stack">
          <textarea
            aria-label="Note"
            className="inspector-note"
            maxLength={2048}
            onBlur={commitNote}
            onChange={(e) => setNoteDraft(e.target.value)}
            placeholder="Add a note about this item"
            rows={3}
            value={noteDraft}
          />
          <div
            className={`inspector-note-counter${noteDraft.length > NOTE_SOFT_LIMIT ? " over-limit" : ""}`}
          >
            {noteDraft.length} / {NOTE_SOFT_LIMIT}
          </div>
        </div>
      </div>
    </section>
  );
}
