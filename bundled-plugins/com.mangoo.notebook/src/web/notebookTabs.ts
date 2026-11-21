export interface NotebookTabs {
  ids: string[];
  activeId: string | null;
}

export function selectTab(layout: NotebookTabs, id: string): NotebookTabs {
  return { ids: layout.ids.includes(id) ? layout.ids : [...layout.ids, id], activeId: id };
}

export function reorderTab(layout: NotebookTabs, id: string, targetId: string, side: "before" | "after"): NotebookTabs {
  const from = layout.ids.indexOf(id);
  const target = layout.ids.indexOf(targetId);
  if (from < 0 || target < 0 || from === target) return layout;
  const destination = target + Number(side === "after") - Number(from < target);
  if (destination === from) return layout;
  const ids = [...layout.ids];
  ids.splice(from, 1);
  ids.splice(destination, 0, id);
  return { ids, activeId: layout.activeId };
}

// Prefer the tab on the right, then the nearest surviving tab on the left.
export function retainTabs(layout: NotebookTabs, keep: (id: string) => boolean): NotebookTabs {
  const ids = layout.ids.filter(keep);
  if (!ids.length) return { ids, activeId: null };
  if (layout.activeId && ids.includes(layout.activeId)) return { ids, activeId: layout.activeId };
  const index = layout.activeId ? layout.ids.indexOf(layout.activeId) : 0;
  return { ids, activeId: layout.ids.slice(Math.max(0, index)).find(keep) ?? ids[ids.length - 1] };
}

export function readTabs(libraryId: string, rows: { id: string }[]): NotebookTabs {
  const live = new Set(rows.map(row => row.id));
  try {
    const saved = JSON.parse(sessionStorage.getItem("mangoo.notebook.tabs:" + libraryId) ?? "null");
    if (Array.isArray(saved?.ids)) {
      const ids = [...new Set<string>(saved.ids.filter((id: unknown) => typeof id === "string"))];
      return retainTabs({ ids, activeId: saved.activeId }, id => live.has(id));
    }
  } catch { /* Storage may be unavailable. */ }
  let previous: string | null = null;
  try { previous = localStorage.getItem("mangoo.notebook.selection:" + libraryId); } catch { /* Optional migration. */ }
  const id = previous && live.has(previous) ? previous : rows[0]?.id;
  return id ? { ids: [id], activeId: id } : { ids: [], activeId: null };
}

export function writeTabs(libraryId: string, layout: NotebookTabs) {
  try { sessionStorage.setItem("mangoo.notebook.tabs:" + libraryId, JSON.stringify(layout)); }
  catch { /* Browsing still works with storage disabled. */ }
}
