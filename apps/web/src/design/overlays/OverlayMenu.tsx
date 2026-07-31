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
    if (!autoFocus) return;
    let frame = 0;
    const focusInitialItem = () => {
      const container = containerRef.current;
      if (!container) return;
      const surface = container.closest<HTMLElement>("[data-overlay-layer-kind]");
      if (surface?.style.visibility === "hidden") {
        frame = window.requestAnimationFrame(focusInitialItem);
        return;
      }
      rovingItemRef.current = synchronizeRovingItems(
        container,
        rovingItemRef.current,
        true
      );
    };
    frame = window.requestAnimationFrame(focusInitialItem);
    return () => window.cancelAnimationFrame(frame);
  }, [autoFocus]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observer = new MutationObserver(() => {
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
    });
    observer.observe(container, {
      attributeFilter: ["aria-checked", "aria-disabled", "data-selected", "disabled"],
      attributes: true,
      childList: true,
      subtree: true
    });
    return () => observer.disconnect();
  }, []);

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
