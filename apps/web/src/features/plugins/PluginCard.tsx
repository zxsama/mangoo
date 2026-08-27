import type { HostPluginProjection } from "@megle/core-client";
import { Cloud, Loader2, Plug } from "lucide-react";
import type { KeyboardEvent } from "react";
import { StatusPill } from "./StatusPill";
import { ONEDRIVE_PLUGIN_ID } from "./pluginAuthModel";

interface PluginCardProps {
  plugin: HostPluginProjection;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onToggleEnabled: () => void;
}

export function pluginTabId(pluginId: string) {
  return `plugin-tab-${pluginId}`;
}

export function pluginPanelId(pluginId: string) {
  return `plugin-panel-${pluginId}`;
}

function handleTabKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
  const tabList = event.currentTarget.closest(".plugins-list");
  const tabs = tabList ? [...tabList.querySelectorAll<HTMLButtonElement>('.plugin-card-body[role="tab"]')] : [];
  const currentIndex = tabs.indexOf(event.currentTarget);
  if (currentIndex < 0 || tabs.length < 2) return;

  let nextIndex: number | null = null;
  if (event.key === "ArrowDown" || event.key === "ArrowRight") nextIndex = (currentIndex + 1) % tabs.length;
  if (event.key === "ArrowUp" || event.key === "ArrowLeft") nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
  if (event.key === "Home") nextIndex = 0;
  if (event.key === "End") nextIndex = tabs.length - 1;
  if (nextIndex === null) return;

  event.preventDefault();
  tabs[nextIndex]?.focus();
  tabs[nextIndex]?.click();
}

export function PluginCard({
  plugin,
  selected,
  busy,
  onSelect,
  onToggleEnabled
}: PluginCardProps) {
  const toggleDisabled = busy || plugin.runtimeState === "incompatible";
  const toggleLabel = plugin.enabledIntent ? "Disable" : "Enable";
  const errorTooltip = plugin.lastErrorDetail ?? undefined;
  const PluginIcon = plugin.pluginId === ONEDRIVE_PLUGIN_ID ? Cloud : Plug;

  return (
    <div
      className={`plugin-card${selected ? " selected" : ""}`}
      data-pointer-glow="off"
      role="presentation"
    >
      <button
        aria-controls={selected ? pluginPanelId(plugin.pluginId) : undefined}
        className="plugin-card-body"
        data-direct-contact-feedback="foreground"
        onClick={onSelect}
        onKeyDown={handleTabKeyDown}
        role="tab"
        aria-selected={selected}
        id={pluginTabId(plugin.pluginId)}
        tabIndex={selected ? 0 : -1}
        type="button"
        title={plugin.name}
      >
        <span className="plugin-card-icon" data-direct-contact-content>
          <PluginIcon aria-hidden="true" size={16} strokeWidth={1.8} />
        </span>
        <span className="plugin-card-name" data-direct-contact-content>{plugin.name}</span>
        <span className="plugin-card-version">v{plugin.selectedVersion}</span>
        <StatusPill status={plugin.runtimeState} title={errorTooltip} />
      </button>
      <div className="plugin-card-actions">
        <label
          className={`plugin-toggle${plugin.enabledIntent ? " on" : ""}${
            toggleDisabled ? " disabled" : ""
          }`}
          aria-disabled={toggleDisabled}
          title={
            plugin.runtimeState === "incompatible"
              ? plugin.compatibilityReason ?? "Incompatible provider"
              : toggleLabel
          }
        >
          <input
            type="checkbox"
            checked={plugin.enabledIntent}
            disabled={toggleDisabled}
            onChange={(event) => {
              event.stopPropagation();
              onToggleEnabled();
            }}
            aria-label={toggleLabel}
          />
          <span className="plugin-toggle-track" aria-hidden="true">
            <span className="plugin-toggle-thumb" />
          </span>
          {busy ? <Loader2 aria-hidden="true" className="plugin-toggle-busy spin" size={12} /> : null}
        </label>
      </div>
    </div>
  );
}
