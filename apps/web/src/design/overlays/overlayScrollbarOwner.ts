// The global scrollbar lives under body, outside its scroll owner's DOM tree.
// Only controller-registered elements can borrow that owner's containment.
const owners = new WeakMap<EventTarget, () => HTMLElement | null>();

export function registerOverlayScrollbarOwner(
  targets: readonly HTMLElement[],
  getOwner: () => HTMLElement | null
): () => void {
  for (const target of targets) owners.set(target, getOwner);
  return () => {
    for (const target of targets) owners.delete(target);
  };
}

export function resolveOverlayScrollbarOwner(target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof HTMLElement) || !target.isConnected) return null;
  const owner = owners.get(target)?.();
  if (!owner?.isConnected || owner.getClientRects().length === 0) return null;
  const style = getComputedStyle(owner);
  return style.visibility === "hidden" || style.display === "none" ? null : owner;
}
