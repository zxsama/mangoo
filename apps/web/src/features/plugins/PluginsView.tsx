import { AlertTriangle } from "../../design/icons";
import { MediaLoadingRing } from "../loading/MediaLoadingRing";
import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader } from "../../design/overlays";
import { PluginCard } from "./PluginCard";
import { PluginDetail } from "./PluginDetail";
import { usePluginsData } from "./usePluginsData";

export function PluginsView() {
  const plugins = usePluginsData();
  const selected = plugins.selectedPlugin;
  const totalCount = plugins.plugins.length;

  return (
    <section className="workspace simple-workspace" aria-label="Plugins workbench">
      <div className="plugins-body">
        <div className="plugins-layout">
          <aside className="plugins-sidebar">
            {plugins.error ? (
              <div className="plugins-error" role="alert">
                <AlertTriangle aria-hidden="true" size={14} />
                <span>{plugins.error}</span>
              </div>
            ) : null}

            {plugins.lastDiscovery && plugins.lastDiscovery.errors.length > 0 ? (
              <div className="plugins-discovery-errors" role="alert">
                <AlertTriangle aria-hidden="true" size={14} />
                <span className="plugins-discovery-summary">
                  {plugins.lastDiscovery.errors.length} installed plugin
                  {plugins.lastDiscovery.errors.length === 1 ? "" : "s"} could not be loaded.
                </span>
              </div>
            ) : null}

            <div
              aria-label="Installed plugins"
              aria-orientation="vertical"
              className="plugins-list"
              role="tablist"
            >
              {plugins.loading && totalCount === 0 ? (
                <div aria-label="Refreshing plugins" className="empty-panel empty-panel-busy" role="status">
                  <MediaLoadingRing size={18} />
                </div>
              ) : totalCount === 0 ? (
                <div className="plugins-empty">
                  <p className="plugins-empty-title">No plugins discovered.</p>
                  <p className="plugins-empty-hint">
                    Trusted bundled plugins are detected automatically.
                  </p>
                </div>
              ) : (
                plugins.plugins.map((plugin) => (
                  <PluginCard
                    key={plugin.pluginId}
                    onSelect={() => plugins.selectPlugin(plugin.pluginId)}
                    plugin={plugin}
                    selected={plugins.selectedPluginId === plugin.pluginId}
                  />
                ))
              )}
            </div>
          </aside>
          <LiquidGlassSurface
            as="div"
            className="plugins-detail-pane"
            interactive
            scrollable
            tone="panel"
          >
            {selected ? (
              <PluginDetail
                busy={plugins.busyPluginIds.has(selected.pluginId)}
                onApprovePermissions={() => void plugins.approvePermissions(selected.pluginId)}
                onToggleEnabled={() => {
                  if (selected.enabledIntent) {
                    void plugins.disable(selected.pluginId);
                  } else {
                    void plugins.enable(selected.pluginId);
                  }
                }}
                onRetry={() => void plugins.retry(selected.pluginId)}
                onRemove={() => void plugins.remove(selected.pluginId)}
                onRefresh={plugins.refresh}
                plugin={selected}
              />
            ) : (
              <div className="plugins-detail-empty">
                {totalCount === 0
                  ? "Installed plugins will appear here automatically."
                  : "Select a plugin to inspect capabilities, permissions, and manifest details."}
              </div>
            )}
          </LiquidGlassSurface>
        </div>
      </div>
      <ModalSurface
        ariaDescribedBy="plugin-removal-error-description"
        ariaLabelledBy="plugin-removal-error-title"
        id="plugin-removal-error-dialog"
        onDismiss={plugins.dismissRemovalError}
        open={Boolean(plugins.removalError)}
        role="alertdialog"
        rootId="library"
      >
        <OverlayDialogHeader
          description="需要先解除使用关系"
          descriptionId="plugin-removal-error-description"
          title="无法移除插件"
          titleId="plugin-removal-error-title"
        />
        <OverlayDialogBody>
          <p className="overlay-dialog-text">{plugins.removalError}</p>
        </OverlayDialogBody>
        <OverlayDialogActions>
          <LiquidGlassButton onClick={plugins.dismissRemovalError} tone="primary" type="button">知道了</LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>
    </section>
  );
}
