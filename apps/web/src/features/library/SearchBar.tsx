import { Search, X } from "../../design/icons";
import { useRef } from "react";
import { OverlayField } from "../../design/overlays";

interface SearchBarProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
}

export function SearchBar({ value, onChange, placeholder = "Search library…" }: SearchBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const clearButton = value ? (
    <button
      aria-label="Clear search"
      className="search-bar-clear"
      onClick={() => {
        onChange("");
        inputRef.current?.focus();
      }}
      type="button"
    >
      <X aria-hidden="true" size={13} />
    </button>
  ) : undefined;

  return (
    <div
      className="search-bar no-drag"
      data-titlebar-control="library-search"
      data-titlebar-search="true"
    >
      <OverlayField
        aria-label="Search library"
        inputRef={inputRef}
        leadingIcon={<Search aria-hidden="true" size={15} />}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        trailingAction={clearButton}
        type="search"
        value={value}
      />
    </div>
  );
}
