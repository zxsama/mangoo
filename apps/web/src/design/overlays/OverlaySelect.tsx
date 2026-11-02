import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type SelectHTMLAttributes
} from "react";
import { ChevronDown } from "../icons";
import { AnchoredOverlay } from "./AnchoredOverlay";
import { useOverlayStack } from "./OverlayProvider";
import { findOverlayMenuTypeaheadMatch, resolveOverlayMenuKey } from "./overlayMenuNavigation";
import type { OverlayRootId } from "./overlayTypes";

export interface OverlaySelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label: string;
}

interface SelectOption {
  index: number;
  label: string;
  disabled: boolean;
  hidden: boolean;
  group: string | null;
  groupIndex: number;
}

interface SelectSnapshot {
  selectedIndex: number;
  options: SelectOption[];
}

// Read the actual form control, including fragments/custom option components and
// browser selection defaults. Do not maintain a second parser for React children.
function readSelect(select: HTMLSelectElement): SelectSnapshot {
  const groups = Array.from(select.getElementsByTagName("optgroup"));
  return {
    selectedIndex: select.selectedIndex,
    options: Array.from(select.options, (option, index) => {
      const group = option.parentElement instanceof HTMLOptGroupElement ? option.parentElement : null;
      return {
        index,
        label: option.label,
        disabled: option.disabled || Boolean(group?.disabled),
        hidden: option.hidden || Boolean(group?.hidden),
        group: group?.label ?? null,
        groupIndex: group ? groups.indexOf(group) : -1
      };
    })
  };
}

function sameSnapshot(a: SelectSnapshot, b: SelectSnapshot): boolean {
  return a.selectedIndex === b.selectedIndex && a.options.length === b.options.length &&
    a.options.every((option, index) => {
      const next = b.options[index];
      return option.label === next.label && option.disabled === next.disabled &&
        option.hidden === next.hidden && option.group === next.group && option.groupIndex === next.groupIndex;
    });
}

export function OverlaySelect(props: OverlaySelectProps) {
  // Multiple/size listboxes are an explicitly separate native form primitive;
  // they never open a native dropdown and retain their multi-selection contract.
  return props.multiple || (props.size ?? 0) > 1
    ? <OverlaySelectListbox {...props} />
    : <OverlaySelectDropdown {...props} />;
}

function OverlaySelectListbox({ className, id, label, ...props }: OverlaySelectProps) {
  const generatedId = useId();
  const selectId = id ?? `overlay-select-${generatedId}`;
  return (
    <div className="overlay-field overlay-select-field overlay-select-listbox-field">
      <label className="overlay-field-label" htmlFor={selectId}>{label}</label>
      <div className="overlay-field-control">
        <select {...props} className={`overlay-field-select${className ? ` ${className}` : ""}`} id={selectId} />
      </div>
    </div>
  );
}

function OverlaySelectDropdown({
  className, id, label, children, onChange, onClick, onKeyDown, onKeyUp,
  onMouseDown, onPointerDown, ...props
}: OverlaySelectProps) {
  const generatedId = useId();
  const selectId = id ?? `overlay-select-${generatedId}`;
  const popupId = `${selectId}-popup`;
  const selectRef = useRef<HTMLSelectElement>(null);
  const { getContainingLayer } = useOverlayStack();
  const [owner, setOwner] = useState<{ rootId: OverlayRootId; parentId?: string }>({ rootId: "select" });
  const [open, setOpen] = useState(false);
  const [present, setPresent] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [snapshot, setSnapshot] = useState<SelectSnapshot>({ selectedIndex: -1, options: [] });
  const typeahead = useRef({ text: "", at: 0 });
  const close = useCallback(() => setOpen(false), []);
  const sync = useCallback(() => {
    const select = selectRef.current;
    if (!select) return;
    const next = readSelect(select);
    setSnapshot((current) => sameSnapshot(current, next) ? current : next);
    setActiveIndex((current) => {
      if (next.options[current] && !next.options[current].disabled && !next.options[current].hidden) return current;
      return next.options.find((option) => !option.disabled && !option.hidden)?.index ?? -1;
    });
    if (select.matches(":disabled")) setOpen(false);
  }, []);

  // React commits value/defaultValue and changed option children before this read.
  useLayoutEffect(sync);
  useEffect(() => {
    const select = selectRef.current;
    if (!select) return;
    const observer = new MutationObserver(sync);
    observer.observe(select, { subtree: true, childList: true, characterData: true, attributes: true,
      attributeFilter: ["label", "disabled", "hidden", "selected", "value"] });
    return () => observer.disconnect();
  }, [sync]);

  useEffect(() => {
    const select = selectRef.current;
    const form = select?.form;
    if (!select || !form) return;
    let mounted = true;
    const reset = (event: Event) => {
      queueMicrotask(() => {
        if (!mounted || event.defaultPrevented) return;
        if (props.value !== undefined) {
          // A controlled select still belongs to its caller after a form reset.
          const value = String(Array.isArray(props.value) ? props.value[0] ?? "" : props.value);
          select.value = value;
        }
        close();
        sync();
      });
    };
    form.addEventListener("reset", reset);
    return () => { mounted = false; form.removeEventListener("reset", reset); };
  }, [close, props.form, props.value, sync]);

  useEffect(() => {
    if (open && activeIndex >= 0) {
      document.getElementById(`${popupId}-option-${activeIndex}`)?.scrollIntoView({ block: "nearest" });
    }
  }, [activeIndex, open, popupId, present]);

  const show = () => {
    const select = selectRef.current;
    if (!select || select.matches(":disabled")) return;
    // Label/assistive activation may dispatch click without pointerdown. Focus
    // the real trigger first so the shared stack dismisses a sibling dropdown.
    select.focus();
    const parent = getContainingLayer(select);
    setOwner(parent ? { rootId: parent.rootId, parentId: parent.id } : { rootId: "select" });
    const next = readSelect(select);
    setSnapshot(next);
    const selected = next.options[next.selectedIndex];
    setActiveIndex(selected && !selected.disabled && !selected.hidden
      ? selected.index : next.options.find((option) => !option.disabled && !option.hidden)?.index ?? -1);
    typeahead.current = { text: "", at: 0 };
    setOpen(true);
  };

  const commit = (index: number) => {
    const select = selectRef.current;
    if (!select || select.matches(":disabled")) return;
    const option = readSelect(select).options[index];
    if (!option || option.disabled || option.hidden) return;
    if (select.selectedIndex !== index) {
      select.selectedIndex = index;
      // Native events on the real select preserve target/currentTarget, form
      // bubbling and React's controlled-value restoration without fake events.
      select.dispatchEvent(new Event("input", { bubbles: true }));
      select.dispatchEvent(new Event("change", { bubbles: true }));
      sync();
    }
    close();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLSelectElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented || event.currentTarget.matches(":disabled")) return;
    if (event.key === "Tab") {
      if (open) commit(activeIndex);
      close();
      return;
    }
    if (event.key === "Escape") {
      // Let the shared stack dismiss only this layer, including modal children.
      if (open) event.preventDefault();
      return;
    }
    const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
    const navigation = ["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Enter", " ", "F4"].includes(event.key);
    if (!printable && !navigation) return;
    event.preventDefault();
    if (event.key === "F4" || (event.altKey && event.key === "ArrowUp")) {
      if (open) close(); else show();
      return;
    }
    const enabled = snapshot.options.filter((option) => !option.disabled && !option.hidden);
    const current = enabled.findIndex((option) => option.index === (open ? activeIndex : snapshot.selectedIndex));
    if (!open) show();
    const action = resolveOverlayMenuKey({ key: event.key, activeIndex: current,
      itemCount: enabled.length, activeHasSubmenu: false, nested: false });
    if (action.type === "activate") {
      if (open) commit(activeIndex);
      return;
    }
    if (action.type === "focus") {
      if (open || event.key === "Home" || event.key === "End") setActiveIndex(enabled[action.index].index);
      return;
    }
    if (!printable) return;
    const now = performance.now();
    typeahead.current.text = (now - typeahead.current.at < 500 ? typeahead.current.text : "") + event.key;
    typeahead.current.at = now;
    const match = findOverlayMenuTypeaheadMatch(enabled.map((option) => option.label), typeahead.current.text, current);
    if (match !== null) setActiveIndex(enabled[match].index);
  };

  const covered = present;
  const groups: { key: number; label: string | null; options: SelectOption[] }[] = [];
  for (const option of snapshot.options.filter((item) => !item.hidden)) {
    const previous = groups.at(-1);
    if (previous && previous.key === option.groupIndex) previous.options.push(option);
    else groups.push({ key: option.groupIndex, label: option.group, options: [option] });
  }
  const renderOption = (option: SelectOption) => (
    <button
      aria-disabled={option.disabled || undefined}
      aria-selected={option.index === snapshot.selectedIndex}
      className="overlay-select-option"
      data-active={option.index === activeIndex || undefined}
      disabled={option.disabled}
      id={`${popupId}-option-${option.index}`}
      key={option.index}
      onClick={() => commit(option.index)}
      onPointerDown={(event) => event.preventDefault()}
      onMouseDown={(event) => event.preventDefault()}
      onPointerEnter={() => { if (!option.disabled) setActiveIndex(option.index); }}
      role="option"
      tabIndex={-1}
      type="button"
    >{option.label}</button>
  );

  return (
    <div className="overlay-field overlay-select-field" data-disabled={props.disabled || undefined}>
      <label className="overlay-field-label" htmlFor={selectId}>{label}</label>
      <div className="overlay-field-control overlay-select-control" data-covered={covered || undefined}
        aria-disabled={props.disabled || undefined}
        data-interactive-pointer-owner={!covered ? "true" : undefined}
        data-pointer-glow={covered ? "off" : undefined}>
        <select
          {...props}
          aria-activedescendant={open && activeIndex >= 0 ? `${popupId}-option-${activeIndex}` : undefined}
          aria-controls={open ? popupId : undefined}
          aria-expanded={open}
          aria-haspopup="listbox"
          className={`overlay-field-select overlay-select-trigger${className ? ` ${className}` : ""}`}
          data-pointer-glow={covered ? "off" : undefined}
          id={selectId}
          onChange={(event) => { onChange?.(event); sync(); }}
          onClick={(event) => {
            onClick?.(event);
            const cancelled = event.defaultPrevented;
            event.preventDefault();
            if (!cancelled) { if (open) close(); else show(); }
          }}
          onKeyDown={handleKeyDown}
          onKeyUp={(event) => {
            onKeyUp?.(event);
            if ([" ", "Enter", "F4"].includes(event.key)) event.preventDefault();
          }}
          onPointerDown={(event) => {
            onPointerDown?.(event);
            const cancelled = event.defaultPrevented;
            event.preventDefault();
            if (!cancelled && event.button === 0) event.currentTarget.focus();
          }}
          onMouseDown={(event) => {
            onMouseDown?.(event);
            const cancelled = event.defaultPrevented;
            event.preventDefault();
            if (!cancelled && event.button === 0) event.currentTarget.focus();
          }}
          ref={selectRef}
        >{children}</select>
        <ChevronDown aria-hidden="true" className="overlay-select-chevron" size={14} />
      </div>
      <AnchoredOverlay
        anchor={selectRef.current}
        ariaLabel={props["aria-label"] ?? label}
        className="overlay-select-popup"
        id={popupId}
        integratedHeader={
          <div aria-hidden="true" className="overlay-select-header"
            onPointerDown={(event) => event.preventDefault()}
            onMouseDown={(event) => event.preventDefault()}
            onClick={close}>
            <span>{snapshot.options[snapshot.selectedIndex]?.label ?? ""}</span>
            <ChevronDown size={14} />
          </div>
        }
        interactivePointerOwner
        onDismiss={close}
        onPresenceChange={setPresent}
        open={open}
        parentId={owner.parentId}
        restoreFocus={false}
        role="listbox"
        rootId={owner.rootId}
      >
        <div className="overlay-select-options">
          {groups.map((group, index) => group.label === null
            ? <div key={index} role="presentation">{group.options.map(renderOption)}</div>
            : <div aria-label={group.label} key={index} role="group">
                <div aria-hidden="true" className="overlay-select-group-label">{group.label}</div>
                {group.options.map(renderOption)}
              </div>)}
        </div>
      </AnchoredOverlay>
    </div>
  );
}
