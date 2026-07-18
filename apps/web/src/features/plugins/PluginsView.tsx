import { useCallback, useState } from "react";
import { AlertTriangle, Loader2, RefreshCw } from "lucide-react";
import { MediaLoadingRing } from "../loading/MediaLoadingRing";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import { PluginCard } from "./PluginCard";
import { PluginDetail } from "./PluginDetail";
import { usePluginsData } from "./usePluginsData";

export function PluginsView() {
  const plugins = usePluginsData();
  const [discovering, setDiscovering] = useState(false);

  const runDiscover = useCallback(async () => {
    setDiscovering(true);
    try {
      await plugins.discover();
    } finally {
      setDiscovering(false);
    }
  }, [plugins]);

  const selected = plugins.selectedPlugin;
  const totalCount = plugins.plugins.length;
  const enabledCount = plugins.plugins.filter((plugin) => plugin.enabled).length;
  const invalidCount = plugins.plugins.filter((plugin) => plugin.status === "invalid").length;

  return (
    <section className="workspace simple-workspace" aria-label="Plugins workbench">
      <div className="plugins-body">
        <div className="plugins-body-actions">
          <div className="plugins-status-summary">
            {totalCount === 0
              ? "No plugins discovered"
              : `${totalCount} registered · ${enabledCount} enabled${
                  invalidCount > 0 ? ` · ${invalidCount} invalid` : ""
                }`}
          </div>
          <button
            className="plugins-rescan"
            onClick={() => void runDiscover()}
            type="button"
            disabled={discovering}
            aria-label="Re-scan plugin folder"
          >
            {discovering ? (
              <Loader2 className="spin" size={14} />
            ) : (
              <RefreshCw size={14} />
            )}
            <span>{discovering ? "Scanning…" : "Re-scan"}</span>
          </button>
        </div>

        {plugins.error ? (
          <div className="plugins-error" role="alert">
            <AlertTriangle aria-hidden="true" size={14} />
            <span>{plugins.error}</span>
          </div>
        ) : null}

        {plugins.lastDiscovery && plugins.lastDiscovery.errors.length > 0 ? (
          <div className="plugins-discovery-errors" role="status">
            <div className="plugins-discovery-summary">
              Last scan: {plugins.lastDiscovery.discovered} discovered,{" "}
              {plugins.lastDiscovery.errors.length} error
              {plugins.lastDiscovery.errors.length === 1 ? "" : "s"}
            </div>
            <ul className="plugins-discovery-error-list">
              {plugins.lastDiscovery.errors.map((entry, index) => (
                <li key={`${entry.manifestPath}-${index}`}>
                  <code title={entry.manifestPath}>{entry.manifestPath}</code>
                  <span>{entry.message}</span>
                </li>
              ))}
            </ul>
          </div>
        ) : null}

        <div className="plugins-layout">
          <div className="plugins-list" role="list">
            {plugins.loading && totalCount === 0 ? (
              <div aria-label="Refreshing plugins" className="empty-panel empty-panel-busy" role="status">
                <MediaLoadingRing size={18} />
              </div>
            ) : totalCount === 0 ? (
              <div className="plugins-empty">
                <p className="plugins-empty-title">No plugins discovered yet.</p>
                <p className="plugins-empty-hint">
                  Drop a plugin folder under{" "}
                  <code className="dialog-code">MEGLE_PLUGINS_DIR</code> and click Re-scan.
                </p>
              </div>
            ) : (
              plugins.plugins.map((plugin) => (
                <PluginCard
                  busy={plugins.busyPluginIds.has(plugin.id)}
                  key={plugin.id}
                  onSelect={() => plugins.selectPlugin(plugin.id)}
                  onToggleEnabled={() => {
                    if (plugin.enabled) {
                      void plugins.disable(plugin.id);
                    } else {
                      void plugins.enable(plugin.id);
                    }
                  }}
                  plugin={plugin}
                  selected={plugins.selectedPluginId === plugin.id}
                />
              ))
            )}
          </div>
          <LiquidGlassSurface
            as="div"
            className="plugins-detail-pane"
            interactive
            scrollable
            tone="panel"
          >
            {selected ? (
              <PluginDetail
                busy={plugins.busyPluginIds.has(selected.id)}
                onRemove={() => void plugins.remove(selected.id)}
                plugin={selected}
              />
            ) : (
              <div className="plugins-detail-empty">
                {totalCount === 0
                  ? "Once a plugin is registered, its details will appear here."
                  : "Select a plugin to inspect capabilities, permissions, and manifest details."}
              </div>
            )}
          </LiquidGlassSurface>
        </div>
      </div>
    </section>
  );
}
