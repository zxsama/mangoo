import {
  useEffect,
  useLayoutEffect,
  useRef,
  type ButtonHTMLAttributes,
  type FocusEvent,
  type HTMLAttributes,
  type KeyboardEvent,
  type ReactNode
} from "react";
import { useOverlayStack } from "./OverlayProvider";
import {
  findOverlayMenuTypeaheadMatch,
  overlayMenuTabIndexes,
  resolveOverlayMenuKey
} from "./overlayMenuNavigation";

const ENABLED_MENU_ITEM_SELECTOR =
  '[data-overlay-menu-item="true"]:not(:disabled):not([aria-disabled="true"])';

function enabledMenuItems(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(
    container.querySelectorAll<HTMLButtonElement>(ENABLED_MENU_ITEM_SELECTOR)
  );
}

function allMenuItems(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(
    container.querySelectorAll<HTMLButtonElement>('[data-overlay-menu-item="true"]')
  );
}

function disabledMenuItemFromTarget(target: EventTarget | null): HTMLButtonElement | null {
  if (!(target instanceof Element)) return null;
  const item = target.closest<HTMLButtonElement>('[data-overlay-menu-item="true"]');
  if (!item || (!item.disabled && item.getAttribute("aria-disabled") !== "true")) return null;
  return item;
}

function preferredMenuItemIndex(items: HTMLButtonElement[]): number {
  return items.findIndex((item) =>
    item.dataset.selected === "true" || item.getAttribute("aria-checked") === "true"
  );
}

function synchronizeRovingItems(
  container: HTMLElement,
  previousRovingItem: HTMLButtonElement | null,
  focus: boolean
): HTMLButtonElement | null {
  const items = enabledMenuItems(container);
  const activeIndex = items.indexOf(document.activeElement as HTMLButtonElement);
  const previousIndex = items.indexOf(previousRovingItem as HTMLButtonElement);
  const preferredIndex = preferredMenuItemIndex(items);
  const nextIndex = activeIndex >= 0
    ? activeIndex
    : previousIndex >= 0
      ? previousIndex
      : preferredIndex >= 0
        ? preferredIndex
        : 0;
  const tabIndexes = overlayMenuTabIndexes(items.length, nextIndex);
  allMenuItems(container).forEach((item) => {
    item.tabIndex = -1;
  });
  items.forEach((item, itemIndex) => {
    item.tabIndex = tabIndexes[itemIndex] ?? -1;
  });
  const nextItem = items[nextIndex] ?? null;
  if (focus) nextItem?.focus();
  return nextItem;
}

export interface OverlayMenuListProps extends HTMLAttributes<HTMLDivElement> {
  ariaLabel: string;
  autoFocus?: boolean;
  role?: "menu" | "listbox";
}

export interface OverlayMenuItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  checked?: boolean;
  danger?: boolean;
  icon?: ReactNode;
  selected?: boolean;
  secondary?: ReactNode;
  shortcut?: ReactNode;
  submenu?: boolean;
}

export function OverlayMenuList({
  ariaLabel,
  autoFocus = true,
  children,
  className,
  onFocusCapture,
  onKeyDown,
  role = "menu",
  ...props
}: OverlayMenuListProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const rovingItemRef = useRef<HTMLButtonElement | null>(null);
  const typeaheadRef = useRef("");
  const typeaheadTimerRef = useRef<number | null>(null);
  const { dismissTop } = useOverlayStack();

  const activateRovingItem = (
    container: HTMLElement,
    items: HTMLButtonElement[],
    index: number,
    focus: boolean
  ) => {
    const tabIndexes = overlayMenuTabIndexes(items.length, index);
    allMenuItems(container).forEach((item) => {
      item.tabIndex = -1;
    });
    items.forEach((item, itemIndex) => {
      item.tabIndex = tabIndexes[itemIndex] ?? -1;
    });
    rovingItemRef.current = items[index] ?? null;
    if (focus) rovingItemRef.current?.focus();
  };

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    rovingItemRef.current = synchronizeRovingItems(container, null, false);
  }, []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const surface = container.closest<HTMLElement>("[data-overlay-layer-kind]");
    let autoFocusPending = autoFocus;
    let frame = 0;

    const surfaceCanReceiveFocus = () => surface === null || (
      surface.getAttribute("aria-hidden") !== "true" &&
      surface.dataset.overlayState !== "closing" &&
      surface.style.visibility !== "hidden"
    );
    const meaningfulSurfaceFocusExists = () => {
      const activeElement = document.activeElement;
      return surface !== null &&
        activeElement instanceof HTMLElement &&
        surface.contains(activeElement) &&
        activeElement !== surface &&
        activeElement !== container;
    };
    const scheduleAutoFocus = () => {
      if (!autoFocusPending || !surfaceCanReceiveFocus()) return;
      if (meaningfulSurfaceFocusExists()) {
        autoFocusPending = false;
        return;
      }
      if (frame) window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        if (!autoFocusPending || !surfaceCanReceiveFocus()) return;
        if (meaningfulSurfaceFocusExists()) {
          autoFocusPending = false;
          return;
        }
        rovingItemRef.current = synchronizeRovingItems(
          container,
          rovingItemRef.current,
          true
        );
        autoFocusPending = rovingItemRef.current === null;
      });
    };
    const synchronizeItems = () => {
      const previousRovingItem = rovingItemRef.current;
      const enabledItems = enabledMenuItems(container);
      const activeElement = document.activeElement;
      const shouldMoveFocus = previousRovingItem !== null &&
        !enabledItems.includes(previousRovingItem) &&
        (activeElement === previousRovingItem || activeElement === document.body);
      rovingItemRef.current = synchronizeRovingItems(
        container,
        previousRovingItem,
        shouldMoveFocus
      );
      scheduleAutoFocus();
    };
    const itemObserver = new MutationObserver(synchronizeItems);
    itemObserver.observe(container, {
      attributeFilter: ["aria-checked", "aria-disabled", "data-selected", "disabled"],
      attributes: true,
      childList: true,
      subtree: true
    });
    const surfaceObserver = surface ? new MutationObserver(() => {
      if (!surfaceCanReceiveFocus()) {
        autoFocusPending = autoFocus;
        if (frame) {
          window.cancelAnimationFrame(frame);
          frame = 0;
        }
        return;
      }
      scheduleAutoFocus();
    }) : null;
    surfaceObserver?.observe(surface as HTMLElement, {
      attributeFilter: ["aria-hidden", "data-overlay-state", "style"],
      attributes: true
    });
    scheduleAutoFocus();

    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      itemObserver.disconnect();
      surfaceObserver?.disconnect();
    };
  }, [autoFocus]);

  useEffect(() => () => {
    if (typeaheadTimerRef.current !== null) {
      window.clearTimeout(typeaheadTimerRef.current);
    }
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;

    const container = containerRef.current;
    if (!container) return;
    const disabledActivationTarget = disabledMenuItemFromTarget(event.target) ??
      disabledMenuItemFromTarget(document.activeElement);
    if (
      (event.key === "Enter" || event.key === " ") &&
      disabledActivationTarget &&
      container.contains(disabledActivationTarget)
    ) {
      event.preventDefault();
      return;
    }
    const items = enabledMenuItems(container);
    if (items.length === 0) return;

    const activeIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    const activeItem = items[activeIndex] ?? items[0];
    const action = resolveOverlayMenuKey({
      key: event.key,
      activeIndex,
      itemCount: items.length,
      activeHasSubmenu: activeItem?.dataset.submenu === "true",
      nested: container.closest('[data-overlay-layer-kind="nested"]') !== null
    });

    if (action.type === "focus") {
      event.preventDefault();
      activateRovingItem(container, items, action.index, true);
      return;
    }
    if (action.type === "activate" || action.type === "open-submenu") {
      event.preventDefault();
      activeItem?.click();
      return;
    }
    if (action.type === "close-submenu") {
      if (dismissTop()) event.preventDefault();
      return;
    }
    if (event.key.length !== 1 || event.altKey || event.ctrlKey || event.metaKey) return;

    typeaheadRef.current += event.key.toLocaleLowerCase();
    if (typeaheadTimerRef.current !== null) {
      window.clearTimeout(typeaheadTimerRef.current);
    }
    typeaheadTimerRef.current = window.setTimeout(() => {
      typeaheadRef.current = "";
      typeaheadTimerRef.current = null;
    }, 500);
    const nextIndex = findOverlayMenuTypeaheadMatch(
      items.map((item) => item.textContent ?? ""),
      typeaheadRef.current,
      activeIndex
    );
    if (nextIndex === null) return;
    event.preventDefault();
    activateRovingItem(container, items, nextIndex, true);
  };

  const handleFocusCapture = (event: FocusEvent<HTMLDivElement>) => {
    onFocusCapture?.(event);
    if (event.defaultPrevented) return;
    const container = containerRef.current;
    if (!container || !(event.target instanceof HTMLButtonElement)) return;
    const items = enabledMenuItems(container);
    const activeIndex = items.indexOf(event.target);
    if (activeIndex >= 0) activateRovingItem(container, items, activeIndex, false);
  };

  return (
    <div
      {...props}
      aria-label={ariaLabel}
      className={`overlay-menu-list${className ? ` ${className}` : ""}`}
      onFocusCapture={handleFocusCapture}
      onKeyDown={handleKeyDown}
      ref={containerRef}
      role={role}
    >
      {children}
    </div>
  );
}

export function OverlayMenuItem({
  checked,
  children,
  className,
  danger = false,
  disabled,
  icon,
  onClick,
  onPointerDown,
  role,
  secondary,
  selected = false,
  shortcut,
  submenu = false,
  tabIndex = -1,
  type = "button",
  ...props
}: OverlayMenuItemProps) {
  const resolvedRole = role ?? (checked === undefined ? "menuitem" : "menuitemcheckbox");
  const ariaDisabled = props["aria-disabled"];
  const isDisabled = disabled || ariaDisabled === true || ariaDisabled === "true";

  return (
    <button
      {...props}
      aria-checked={checked}
      aria-disabled={ariaDisabled}
      aria-haspopup={submenu ? "menu" : props["aria-haspopup"]}
      aria-selected={selected || undefined}
      className={`overlay-menu-item${className ? ` ${className}` : ""}`}
      data-danger={danger || undefined}
      data-interactive-pointer-target={!isDisabled ? "true" : undefined}
      data-overlay-menu-item="true"
      data-selected={selected || undefined}
      data-submenu={submenu || undefined}
      disabled={disabled}
      onClick={(event) => {
        if (isDisabled) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        onClick?.(event);
      }}
      onPointerDown={(event) => {
        if (isDisabled) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        onPointerDown?.(event);
      }}
      role={resolvedRole}
      tabIndex={isDisabled ? -1 : tabIndex}
      type={type}
    >
      {icon ? <span className="overlay-menu-item-icon" aria-hidden="true">{icon}</span> : null}
      <span className="overlay-menu-item-content">
        <span className="overlay-menu-item-label">{children}</span>
        {secondary ? <span className="overlay-menu-item-secondary">{secondary}</span> : null}
      </span>
      {shortcut ? <span className="overlay-menu-item-shortcut">{shortcut}</span> : null}
      {submenu ? <span className="overlay-menu-item-submenu" aria-hidden="true">{"\u203a"}</span> : null}
    </button>
  );
}
