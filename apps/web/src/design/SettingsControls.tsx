import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { resolveOverlayScrollbarOwner } from "./overlays/overlayScrollbarOwner";
import "./settingsControls.css";

export function SettingsSection({ id, anchorId, title, icon, children }: {
  id: string;
  anchorId?: string;
  title: string;
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-control-section iv-settings-section" data-section={id} id={anchorId}>
      <h3 className="settings-control-heading iv-settings-section-title">
        {icon}<span>{title}</span>
      </h3>
      <div className="settings-control-rows settings-group-body iv-settings-rows">{children}</div>
    </section>
  );
}

export function SettingRow({ title, description, children, htmlFor, stacked = false }: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  htmlFor?: string;
  stacked?: boolean;
}) {
  return (
    <div className="settings-control-row iv-setting-row" data-layout={stacked ? "stacked" : "inline"}>
      <div
        className="settings-control-copy iv-setting-text"
        onClick={(event) => {
          // Labels already activate an explicitly associated control themselves.
          if (htmlFor && (event.target as Element).closest("label")) return;
          const input = event.currentTarget.parentElement?.querySelector<HTMLInputElement>('input[role="switch"]');
          if (input && !input.disabled) input.click();
        }}
      >
        <label className="settings-control-label iv-setting-title" htmlFor={htmlFor}>{title}</label>
        {description && <p className="settings-control-description iv-setting-description">{description}</p>}
      </div>
      <div className="settings-control-value iv-setting-control">{children}</div>
    </div>
  );
}

export function SettingsSwitch({ checked, label, onChange, disabled = false, id }: {
  checked: boolean;
  label: string;
  onChange(next: boolean): void;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <label className={`plugin-toggle settings-control-switch iv-switch${checked ? " on" : ""}${disabled ? " disabled" : ""}`}>
      <input aria-label={label} checked={checked} disabled={disabled} id={id} onChange={(event) => onChange(event.currentTarget.checked)} role="switch" type="checkbox" />
      <span aria-hidden="true" className="plugin-toggle-track">
        <span className="plugin-toggle-thumb" />
      </span>
    </label>
  );
}

export function SettingsSegmented<T extends string | number>({ value, options, label, onChange, disabled = false }: {
  value: T;
  options: ReadonlyArray<{ value: T; label: ReactNode; title?: string }>;
  label: string;
  onChange(next: T): void;
  disabled?: boolean;
}) {
  return (
    <div aria-label={label} className="settings-control-segmented iv-segmented" role="radiogroup">
      {options.map((option, index) => (
        <button
          aria-checked={option.value === value}
          className="settings-control-segment iv-segment"
          data-selected={option.value === value}
          disabled={disabled}
          key={option.value}
          onClick={() => onChange(option.value)}
          onKeyDown={(event) => {
            const direction = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
            if (!direction) return;
            event.preventDefault();
            const next = (index + direction + options.length) % options.length;
            event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
            onChange(options[next].value);
          }}
          role="radio"
          tabIndex={option.value === value ? 0 : -1}
          title={option.title}
          type="button"
        >
          <span data-button-content>{option.label}</span>
        </button>
      ))}
    </div>
  );
}

/** Anchor navigation for a settings column. Only listed section IDs participate,
 * so a category may contain further titled groups or plugin-owned sections. */
export function useSettingsNavigation<T extends string>(sectionIds: readonly T[], initialSection?: T) {
  const [active, setActive] = useState<T>(initialSection ?? sectionIds[0]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const destination = useRef<T | null>(null);
  const syncPosition = useRef<(() => void) | null>(null);
  const jumpTo = useCallback((id: T, smooth = true) => {
    const scroller = scrollRef.current;
    const target = scroller?.querySelector<HTMLElement>(`[data-section="${CSS.escape(id)}"]`);
    if (!scroller || !target) return;
    const animate = smooth && !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    destination.current = id;
    setActive(id);
    const top = target.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - 12;
    scroller.scrollTo({ top: Math.max(0, top), behavior: animate ? "smooth" : "instant" });
    syncPosition.current?.();
  }, []);

  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const ids = new Set<string>(sectionIds);
    let width = scroller.clientWidth;
    let height = scroller.clientHeight;
    let reading: { id: T; offset: number } | undefined;
    const scrollTopFor = (section: HTMLElement, offset: number) => {
      const top = scroller.scrollTop + section.getBoundingClientRect().top - scroller.getBoundingClientRect().top - offset;
      return Math.max(0, Math.min(top, scroller.scrollHeight - scroller.clientHeight));
    };
    const sectionById = (id: T) => scroller.querySelector<HTMLElement>(`[data-section="${CSS.escape(id)}"]`);
    const update = () => {
      if (!height || width !== scroller.clientWidth || height !== scroller.clientHeight) return;
      if (destination.current) {
        const target = sectionById(destination.current);
        if (target && Math.abs(scroller.scrollTop - scrollTopFor(target, 12)) > 1) return;
        destination.current = null;
      }
      const sections = [...scroller.querySelectorAll<HTMLElement>("[data-section]")].filter((section) => ids.has(section.dataset.section!));
      if (!sections.length) return;
      let current = sections[0];
      if (scroller.scrollTop > 0) {
        const edge = scroller.getBoundingClientRect().top + 28;
        const bottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
        for (const section of sections) {
          if (!bottom && section.getBoundingClientRect().top > edge) break;
          current = section;
        }
      }
      const id = current.dataset.section as T;
      reading = { id, offset: current.getBoundingClientRect().top - scroller.getBoundingClientRect().top };
      setActive(id);
    };
    syncPosition.current = update;
    const cancelJump = () => {
      if (!destination.current) return;
      destination.current = null;
      scroller.scrollTo({ top: scroller.scrollTop, behavior: "instant" });
      update();
    };
    const onScrollInput = (event: Event) => {
      if (destination.current && (scroller.contains(event.target as Node) || resolveOverlayScrollbarOwner(event.target) === scroller)) {
        cancelJump();
      }
    };
    const finishJump = () => {
      // A previous scroll can finish after a newer click or viewport change.
      // Settle the current destination before letting the sidebar follow the view.
      if (destination.current && width === scroller.clientWidth && height === scroller.clientHeight) {
        const target = sectionById(destination.current);
        if (target) scroller.scrollTo({ top: scrollTopFor(target, 12), behavior: "instant" });
      }
      update();
    };
    const resize = new ResizeObserver(() => {
      if (width !== scroller.clientWidth || height !== scroller.clientHeight) {
        width = scroller.clientWidth;
        height = scroller.clientHeight;
        // Reflow can move a target by several screens during a long jump.
        // Preserve the current heading and its offset, or align the pending target.
        const position = destination.current ? { id: destination.current, offset: 12 } : reading;
        if (position && height) {
          const section = sectionById(position.id);
          if (section) scroller.scrollTo({ top: scrollTopFor(section, position.offset), behavior: "instant" });
        }
      }
      update();
    });
    resize.observe(scroller);
    if (scroller.firstElementChild) resize.observe(scroller.firstElementChild);
    scroller.addEventListener("scroll", update, { passive: true });
    scroller.addEventListener("scrollend", finishJump);
    document.addEventListener("wheel", onScrollInput, { capture: true, passive: true });
    document.addEventListener("pointerdown", onScrollInput, true);
    scroller.addEventListener("keydown", cancelJump);
    update();
    return () => {
      resize.disconnect();
      syncPosition.current = null;
      scroller.removeEventListener("scroll", update);
      scroller.removeEventListener("scrollend", finishJump);
      document.removeEventListener("wheel", onScrollInput, true);
      document.removeEventListener("pointerdown", onScrollInput, true);
      scroller.removeEventListener("keydown", cancelJump);
      destination.current = null;
    };
  }, [sectionIds]);

  useLayoutEffect(() => {
    if (initialSection) jumpTo(initialSection, false);
  }, [initialSection, jumpTo]);

  return { active, jumpTo, scrollRef };
}
