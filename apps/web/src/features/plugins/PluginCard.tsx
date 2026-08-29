import type { HostPluginProjection } from "@megle/core-client";
import { Cloud, Plug } from "lucide-react";
import type { KeyboardEvent } from "react";
import { StatusPill } from "./StatusPill";
import { ONEDRIVE_PLUGIN_ID } from "./pluginAuthModel";

interface PluginCardProps {
  plugin: HostPluginProjection;
  selected: boolean;
  onSelect: () => void;
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
  onSelect
}: PluginCardProps) {
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
    </div>
  );
}
