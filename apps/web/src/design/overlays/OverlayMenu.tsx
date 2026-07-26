import {
  useEffect,
  useRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type KeyboardEvent,
  type ReactNode
} from "react";

const ENABLED_MENU_ITEM_SELECTOR =
  '[data-overlay-menu-item="true"]:not(:disabled):not([aria-disabled="true"])';

function enabledMenuItems(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(
    container.querySelectorAll<HTMLButtonElement>(ENABLED_MENU_ITEM_SELECTOR)
  );
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
  onKeyDown,
  role = "menu",
  ...props
}: OverlayMenuListProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!autoFocus) return;
    const container = containerRef.current;
    if (!container) return;
    const items = enabledMenuItems(container);
    const preferred = items.find((item) =>
      item.dataset.selected === "true" || item.getAttribute("aria-checked") === "true"
    );
    (preferred ?? items[0])?.focus();
  }, [autoFocus]);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;

    const container = containerRef.current;
    if (!container) return;
    const items = enabledMenuItems(container);
    if (items.length === 0) return;

    const activeIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    let nextIndex: number | null = null;

    if (event.key === "ArrowDown") {
      nextIndex = activeIndex < 0 ? 0 : (activeIndex + 1) % items.length;
    } else if (event.key === "ArrowUp") {
      nextIndex = activeIndex < 0 ? items.length - 1 : (activeIndex - 1 + items.length) % items.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = items.length - 1;
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      (items[activeIndex] ?? items[0]).click();
      return;
    } else {
      return;
    }

    event.preventDefault();
    items[nextIndex]?.focus();
  };

  return (
    <div
      {...props}
      aria-label={ariaLabel}
      className={`overlay-menu-list${className ? ` ${className}` : ""}`}
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
  role,
  secondary,
  selected = false,
  shortcut,
  submenu = false,
  type = "button",
  ...props
}: OverlayMenuItemProps) {
  const resolvedRole = role ?? (checked === undefined ? "menuitem" : "menuitemcheckbox");
  const ariaDisabled = props["aria-disabled"];

  return (
    <button
      {...props}
      aria-checked={checked}
      aria-disabled={ariaDisabled}
      aria-selected={selected || undefined}
      className={`overlay-menu-item${className ? ` ${className}` : ""}`}
      data-danger={danger || undefined}
      data-interactive-pointer-target={
        !disabled && ariaDisabled !== true && ariaDisabled !== "true" ? "true" : undefined
      }
      data-overlay-menu-item="true"
      data-selected={selected || undefined}
      data-submenu={submenu || undefined}
      disabled={disabled}
      role={resolvedRole}
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
