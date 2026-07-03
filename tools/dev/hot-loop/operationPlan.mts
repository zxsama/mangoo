// tools/dev/hot-loop/operationPlan.mts
export const OPERATION_KINDS = [
  "scroll", // deep-scroll: request a media window at an offset
  "folderSwitch", // switch to a real subfolder's first page
  "priorityScope", // POST a rolling selected/visible/ahead scope
  "thumbnailState", // poll thumbnail state for a visible window
  "preview", // open the selected item's original bytes
  "thumbnailBlob" // fetch a ready thumbnail blob
] as const;
export type OperationKind = (typeof OPERATION_KINDS)[number];

export type Operation =
  | { kind: "scroll"; offset: number; limit: number }
  | { kind: "folderSwitch"; folderIndex: number }
  | { kind: "priorityScope"; windowStart: number }
  | { kind: "thumbnailState"; windowStart: number }
  | { kind: "preview"; windowStart: number }
  | { kind: "thumbnailBlob"; windowStart: number };

const WINDOW = 96; // grid page size, matches INITIAL_MEDIA_WINDOW_LIMIT in the UI

// Deterministic weighted mix that mirrors real desktop browsing: scrolling
// dominates, priority-scope + state polling shadow it, preview/blob/folder
// switches punctuate. No RNG so runs are comparable.
export function buildOperationPlan(input: {
  totalOperations: number;
  mediaTotal: number;
}): Operation[] {
  const { totalOperations, mediaTotal } = input;
  const maxOffset = Math.max(0, mediaTotal - WINDOW);
  const ops: Operation[] = [];
  let offset = 0;
  const offsetStep = Math.max(WINDOW, Math.floor(maxOffset / Math.max(1, totalOperations)));
  for (let i = 0; i < totalOperations; i += 1) {
    const cycle = i % 10;
    const windowStart = Math.min(maxOffset, offset);
    if (cycle === 0 || cycle === 2 || cycle === 5 || cycle === 7) {
      ops.push({ kind: "scroll", offset: Math.min(maxOffset, offset), limit: WINDOW });
      offset = Math.min(maxOffset, offset + offsetStep);
    } else if (cycle === 1 || cycle === 6) {
      ops.push({ kind: "priorityScope", windowStart });
    } else if (cycle === 3) {
      ops.push({ kind: "thumbnailState", windowStart });
    } else if (cycle === 4) {
      ops.push({ kind: "folderSwitch", folderIndex: Math.floor(i / 10) });
    } else if (cycle === 8) {
      ops.push({ kind: "thumbnailBlob", windowStart });
    } else {
      ops.push({ kind: "preview", windowStart });
    }
  }
  return ops;
}
