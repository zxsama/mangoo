import type { NotebookFolder, NotebookTree, NoteSummary } from "@mangoo/core-client";

export interface TreeItem {
  id: string;
  name: string;
  parentId: string | null;
  folder?: NotebookFolder;
  note?: NoteSummary;
}

export interface TreeRow {
  item: TreeItem;
  depth: number;
  position: number;
  siblings: number;
}

const collator = new Intl.Collator("zh-CN", { numeric: true });

export function indexNotebook(tree: NotebookTree, notes: NoteSummary[], search: string) {
  const items = new Map<string, TreeItem>();
  const children = new Map<string | null, TreeItem[]>();
  const append = (item: TreeItem) => {
    items.set(item.id, item);
    const siblings = children.get(item.parentId);
    if (siblings) siblings.push(item);
    else children.set(item.parentId, [item]);
  };
  for (const folder of [...tree.folders].sort((a, b) => collator.compare(a.name, b.name) || a.id.localeCompare(b.id))) {
    append({ id: folder.id, name: folder.name, parentId: folder.parentId, folder });
  }
  for (const note of notes) {
    append({ id: note.id, name: note.title || "无标题", parentId: tree.noteParents[note.id] ?? null, note });
  }

  const needle = search.trim().toLocaleLowerCase();
  const matches = new Set<string>();
  if (needle) {
    // The workspace already filtered note titles/body through the Core client.
    for (const item of items.values()) {
      if (item.folder && !item.name.toLocaleLowerCase().includes(needle)) continue;
      let cursor: TreeItem | undefined = item;
      while (cursor && !matches.has(cursor.id)) {
        matches.add(cursor.id);
        cursor = cursor.parentId ? items.get(cursor.parentId) : undefined;
      }
    }
  }
  return { items, children, matches: needle ? matches : null };
}

export function flattenNotebook(
  index: ReturnType<typeof indexNotebook>,
  expanded: ReadonlySet<string>,
): TreeRow[] {
  const rows: TreeRow[] = [];
  const stack: TreeRow[] = [];
  const addChildren = (id: string | null, depth: number) => {
    let siblings = index.children.get(id) ?? [];
    if (index.matches) siblings = siblings.filter((item) => index.matches!.has(item.id));
    for (let i = siblings.length - 1; i >= 0; i--) {
      stack.push({ item: siblings[i], depth, position: i + 1, siblings: siblings.length });
    }
  };
  addChildren(null, 1);
  while (stack.length) {
    const row = stack.pop()!;
    rows.push(row);
    if (row.item.folder && (expanded.has(row.item.id) || index.matches)) addChildren(row.item.id, row.depth + 1);
  }
  return rows;
}

export function moveDestinations(folders: NotebookFolder[], excluded: string | undefined) {
  const tree = indexNotebook({ folders, noteParents: {} }, [], "");
  const choices: { id: string; label: string }[] = [];
  const stack: { item: TreeItem; path: string }[] = [];
  const descend = (parentId: string | null, path: string) => {
    const children = tree.children.get(parentId) ?? [];
    for (let i = children.length - 1; i >= 0; i--) {
      if (children[i].id !== excluded) stack.push({ item: children[i], path });
    }
  };
  descend(null, "");
  while (stack.length) {
    const { item, path } = stack.pop()!;
    const label = path ? path + " / " + item.name : item.name;
    choices.push({ id: item.id, label });
    descend(item.id, label);
  }
  return choices;
}

export function canMoveItem(item: TreeItem, parent: string | null, parents: ReadonlyMap<string, string | null>) {
  if (item.parentId === parent || (parent !== null && !parents.has(parent))) return false;
  for (let id = parent; id !== null; id = parents.get(id) ?? null) {
    if (id === item.id) return false;
  }
  return true;
}
