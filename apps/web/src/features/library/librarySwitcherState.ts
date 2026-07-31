export interface LibraryRemoveTarget {
  name: string;
  rootId: number;
}

export type LibrarySwitcherNestedState =
  | { kind: "closed" }
  | { anchor: HTMLButtonElement; kind: "icon"; libraryId: string }
  | { anchor: HTMLButtonElement; kind: "remove"; libraryId: string };

export type LibraryRemoveState =
  | { kind: "closed" }
  | ({ kind: "confirming" | "pending" } & LibraryRemoveTarget);

export interface LibrarySwitcherInteractionState {
  nested: LibrarySwitcherNestedState;
  remove: LibraryRemoveState;
}

export type LibrarySwitcherInteractionAction =
  | { type: "close-root" }
  | { kind: "icon" | "remove"; type: "dismiss-nested" }
  | { anchor: HTMLButtonElement; kind: "icon" | "remove"; libraryId: string; type: "toggle-nested" }
  | ({ type: "open-remove-confirmation" } & LibraryRemoveTarget)
  | { type: "cancel-remove" }
  | { type: "begin-remove" }
  | { type: "fail-remove" }
  | { type: "complete-remove" };

export const INITIAL_LIBRARY_SWITCHER_INTERACTION_STATE: LibrarySwitcherInteractionState = {
  nested: { kind: "closed" },
  remove: { kind: "closed" },
};

export function librarySwitcherInteractionReducer(
  state: LibrarySwitcherInteractionState,
  action: LibrarySwitcherInteractionAction,
): LibrarySwitcherInteractionState {
  switch (action.type) {
    case "close-root":
      return INITIAL_LIBRARY_SWITCHER_INTERACTION_STATE;
    case "toggle-nested":
      if (state.remove.kind === "pending") return state;
      if (
        state.nested.kind === action.kind &&
        state.nested.libraryId === action.libraryId
      ) {
        return { ...state, nested: { kind: "closed" } };
      }
      return {
        ...state,
        nested: {
          anchor: action.anchor,
          kind: action.kind,
          libraryId: action.libraryId,
        },
      };
    case "dismiss-nested":
      if (state.remove.kind === "pending" || state.nested.kind !== action.kind) return state;
      return { ...state, nested: { kind: "closed" } };
    case "open-remove-confirmation":
      return {
        ...state,
        remove: {
          kind: "confirming",
          name: action.name,
          rootId: action.rootId,
        },
      };
    case "cancel-remove":
      return state.remove.kind === "confirming"
        ? { ...state, remove: { kind: "closed" } }
        : state;
    case "begin-remove":
      return state.remove.kind === "confirming"
        ? { ...state, remove: { ...state.remove, kind: "pending" } }
        : state;
    case "fail-remove":
      return state.remove.kind === "pending"
        ? { ...state, remove: { ...state.remove, kind: "confirming" } }
        : state;
    case "complete-remove":
      return {
        nested: { kind: "closed" },
        remove: { kind: "closed" },
      };
  }
}
