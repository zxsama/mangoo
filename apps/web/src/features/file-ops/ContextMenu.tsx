import { useMemo, type ReactNode } from "react";
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

  return (
    <AnchoredOverlay
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
              onClick={item.onSelect}
              role="menuitem"
            >
              {item.label}
            </OverlayMenuItem>
          ),
        )}
      </OverlayMenuList>
    </AnchoredOverlay>
  );
}
