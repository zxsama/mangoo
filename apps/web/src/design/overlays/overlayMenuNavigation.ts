export type OverlayMenuKeyAction =
  | { type: "focus"; index: number }
  | { type: "activate" }
  | { type: "open-submenu" }
  | { type: "close-submenu" }
  | { type: "none" };

export interface ResolveOverlayMenuKeyInput {
  key: string;
  activeIndex: number;
  itemCount: number;
  activeHasSubmenu: boolean;
  nested: boolean;
}

export function overlayMenuTabIndexes(itemCount: number, activeIndex: number): number[] {
  if (itemCount <= 0) return [];
  const resolvedIndex = activeIndex >= 0 && activeIndex < itemCount ? activeIndex : 0;
  return Array.from({ length: itemCount }, (_, index) => index === resolvedIndex ? 0 : -1);
}

export function resolveOverlayMenuKey({
  key,
  activeIndex,
  itemCount,
  activeHasSubmenu,
  nested
}: ResolveOverlayMenuKeyInput): OverlayMenuKeyAction {
  if (key === "ArrowLeft" && nested) return { type: "close-submenu" };
  if (key === "ArrowRight" && activeHasSubmenu) return { type: "open-submenu" };
  if (itemCount <= 0) return { type: "none" };

  switch (key) {
    case "ArrowDown":
      return { type: "focus", index: activeIndex < 0 ? 0 : (activeIndex + 1) % itemCount };
    case "ArrowUp":
      return {
        type: "focus",
        index: activeIndex < 0 ? itemCount - 1 : (activeIndex - 1 + itemCount) % itemCount
      };
    case "Home":
      return { type: "focus", index: 0 };
    case "End":
      return { type: "focus", index: itemCount - 1 };
    case "Enter":
    case " ":
      return { type: "activate" };
    default:
      return { type: "none" };
  }
}

export function findOverlayMenuTypeaheadMatch(
  labels: string[],
  query: string,
  activeIndex: number
): number | null {
  if (labels.length === 0) return null;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return null;
  const repeatedCharacter = [...normalizedQuery].every(
    (character) => character === normalizedQuery[0]
  );
  const search = repeatedCharacter ? normalizedQuery[0] : normalizedQuery;

  for (let offset = 1; offset <= labels.length; offset += 1) {
    const index = (Math.max(activeIndex, -1) + offset) % labels.length;
    if (labels[index]?.trim().toLocaleLowerCase().startsWith(search)) return index;
  }
  return null;
}
