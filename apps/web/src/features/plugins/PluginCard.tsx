import type { HostPluginProjection } from "@megle/core-client";
import { Loader2 } from "lucide-react";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import { StatusPill } from "./StatusPill";

interface PluginCardProps {
  plugin: HostPluginProjection;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onToggleEnabled: () => void;
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

  return (
    <LiquidGlassSurface
      as="article"
      active={selected}
      className={`plugin-card${selected ? " selected" : ""}`}
      interactive
      role="listitem"
      aria-label={`${plugin.pluginId} plugin`}
      tone="panel"
    >
      <button
        className="plugin-card-body"
        data-direct-contact-feedback="foreground"
        onClick={onSelect}
        type="button"
        aria-pressed={selected}
      >
        <div className="plugin-card-heading">
          <span className="plugin-card-name" data-direct-contact-content>{plugin.pluginId}</span>
          <span className="plugin-card-version" data-direct-contact-content>v{plugin.selectedVersion}</span>
        </div>
        <div className="plugin-card-meta">
          <StatusPill status={plugin.runtimeState} title={errorTooltip} />
        </div>
      </button>
      <div className="plugin-card-actions">
        <label
          className={`plugin-toggle${plugin.enabledIntent ? " on" : ""}${
            toggleDisabled ? " disabled" : ""
          }`}
          data-direct-contact-feedback="foreground"
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
    </LiquidGlassSurface>
  );
}
