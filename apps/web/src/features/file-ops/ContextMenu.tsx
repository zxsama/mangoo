import { useMemo, useState, type ReactNode } from "react";
import {
  AnchoredOverlay,
  OverlayMenuItem,
  OverlayMenuList,
  pointOverlayAnchor,
} from "../../design/overlays";

export interface ContextMenuItem {
  id: string;
  label?: ReactNode;
  icon?: ReactNode;
  separator?: boolean;
  disabled?: boolean;
  danger?: boolean;
  onSelect?: () => void;
  renderSubmenu?: (anchor: HTMLElement, onClose: () => void) => ReactNode;
}

interface ContextMenuProps {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onDismiss: () => void;
  restoreFocusTarget?: HTMLElement | null;
  /** Accessible label for the menu wrapper (`role="menu"`). */
  ariaLabel?: string;
}

export function ContextMenu({
  x,
  y,
  items,
  onDismiss,
  restoreFocusTarget,
  ariaLabel = "Item actions",
}: ContextMenuProps) {
  const anchor = useMemo(() => pointOverlayAnchor(x, y), [x, y]);
  const [submenu, setSubmenu] = useState<{ item: ContextMenuItem; anchor: HTMLElement } | null>(null);

  return (
    <><AnchoredOverlay
      anchor={anchor}
      ariaLabel={ariaLabel}
      className="context-menu"
      id="context-menu-root"
      onDismiss={onDismiss}
      open
      placement="bottom-start"
      preferredWidth={180}
      restoreFocusTarget={restoreFocusTarget}
      role="group"
      rootId="context-menu"
    >
      <OverlayMenuList ariaLabel={ariaLabel}>
        {items.map((item) =>
          item.separator ? (
            <div className="overlay-menu-separator" key={item.id} role="separator" />
          ) : (
            <OverlayMenuItem
              danger={item.danger}
              disabled={item.disabled}
              icon={item.icon}
              key={item.id}
              submenu={Boolean(item.renderSubmenu)}
              onClick={(event) => {
                if (item.renderSubmenu) {
                  event.currentTarget.focus();
                  setSubmenu({ item, anchor: event.currentTarget });
                } else item.onSelect?.();
              }}
              onKeyDown={(event) => {
                if (item.renderSubmenu && event.key === "ArrowRight") {
                  event.preventDefault(); event.stopPropagation();
                  setSubmenu({ item, anchor: event.currentTarget });
                }
              }}
              role="menuitem"
            >
              {item.label}
            </OverlayMenuItem>
          ),
        )}
      </OverlayMenuList>
    </AnchoredOverlay>
    {submenu?.item.renderSubmenu?.(submenu.anchor, () => setSubmenu(null))}</>
  );
}
