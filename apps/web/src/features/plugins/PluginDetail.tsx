import { ChevronRight, Database, Loader2, Trash2 } from "../../design/icons";
import type { HostPluginProjection, PluginCapability } from "@mangoo/core-client";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { StatusPill } from "./StatusPill";
import { PluginAccountSetup } from "./PluginAccountSetup";
import { pluginPanelId, pluginTabId } from "./PluginCard";
import { PluginContributionsSlot } from "./PluginContributionsSlot";

interface PluginDetailProps {
  plugin: HostPluginProjection;
  busy: boolean;
  onApprovePermissions: () => void;
  onToggleEnabled: () => void;
  onRetry: () => void;
  onRemove: () => void;
  onRefresh: () => void | Promise<void>;
}

const CAPABILITY_LABEL: Record<PluginCapability, string> = {
  "sync-provider.v1": "Sync provider v1",
  "file-decoder.v1": "文件解析"
};

export function PluginDetail({
  plugin,
  busy,
  onApprovePermissions,
  onToggleEnabled,
  onRetry,
  onRemove,
  onRefresh
}: PluginDetailProps) {
  const hasSetup = !plugin.contributions?.ui && Boolean(plugin.contributions?.configuration.length || plugin.contributions?.authentication);
  const toggleDisabled = busy || plugin.runtimeState === "incompatible" || plugin.runtimeState === "stopping";
  const toggleLabel = plugin.enabledIntent ? "停用插件" : "启用插件";

  return (
    <section
      aria-labelledby={pluginTabId(plugin.pluginId)}
      className="plugin-detail"
      id={pluginPanelId(plugin.pluginId)}
      role="tabpanel"
    >
      <header className="plugin-detail-header">
        <div className="plugin-detail-heading">
          <div className="plugin-detail-titles">
            <div className="plugin-detail-title">{plugin.name}</div>
            <div className="plugin-detail-subtitle">
              <span>v{plugin.selectedVersion}</span>
            </div>
            <div className="plugin-detail-enable-row">
              <span>启用插件</span>
              <label
                aria-disabled={toggleDisabled}
                className={`plugin-toggle${plugin.enabledIntent ? " on" : ""}${toggleDisabled ? " disabled" : ""}`}
                title={plugin.runtimeState === "incompatible" ? plugin.compatibilityReason ?? "插件不兼容" : toggleLabel}
              >
                <input
                  aria-label={toggleLabel}
                  checked={plugin.enabledIntent}
                  disabled={toggleDisabled}
                  onChange={onToggleEnabled}
                  type="checkbox"
                />
                <span aria-hidden="true" className="plugin-toggle-track">
                  <span className="plugin-toggle-thumb" />
                </span>
                {busy ? <Loader2 aria-hidden="true" className="plugin-toggle-busy spin" size={12} /> : null}
              </label>
            </div>
          </div>
          {!hasSetup ? (
            <StatusPill status={plugin.runtimeState} title={plugin.lastErrorDetail ?? undefined} />
          ) : null}
        </div>
      </header>

      {hasSetup ? (
        <div className="plugin-detail-section plugin-detail-primary-section">
          <PluginAccountSetup key={plugin.pluginId} pluginId={plugin.pluginId} onChanged={onRefresh} />
        </div>
      ) : null}
      <PluginContributionsSlot slot="plugin-detail" pluginId={plugin.pluginId} />

      {(plugin.permissionRequired || plugin.runtimeState === "crashed-backoff" || plugin.runtimeState === "degraded") ? (
        <div className="plugin-primary-actions">
          {plugin.permissionRequired && !hasSetup ? (
            <LiquidGlassButton className="plugin-action" disabled={busy} onClick={onApprovePermissions} tone="primary" type="button">
              Approve permissions
            </LiquidGlassButton>
          ) : null}
          {plugin.runtimeState === "crashed-backoff" || plugin.runtimeState === "degraded" ? (
            <LiquidGlassButton className="plugin-action" disabled={busy} onClick={onRetry} tone="primary" type="button">
              Retry provider
            </LiquidGlassButton>
          ) : null}
        </div>
      ) : null}

      <details className="plugin-disclosure plugin-technical-details">
        <summary className="plugin-disclosure-summary" data-pointer-glow="off">
          <ChevronRight aria-hidden="true" className="plugin-disclosure-chevron" size={15} />
          <span className="plugin-disclosure-label">技术信息</span>
        </summary>
        <div className="plugin-disclosure-body">
          <div className="plugin-detail-section">
            <div className="plugin-detail-section-title">Capabilities</div>
            {plugin.capabilities.length === 0 ? (
              <div className="plugin-detail-empty">None declared.</div>
            ) : (
              <div className="plugin-capability-chips">
                {plugin.capabilities.map((capability) => (
                  <span className="plugin-capability-chip" key={capability}>
                    <Database size={12} aria-hidden="true" />
                    <span>{CAPABILITY_LABEL[capability]}</span>
                  </span>
                ))}
              </div>
            )}
          </div>

          <div className="plugin-detail-section">
            <div className="plugin-detail-section-title">Permissions</div>
            {plugin.requestedPermissions.length === 0 ? (
              <div className="plugin-detail-empty">No permissions requested.</div>
            ) : (
              <ul className="plugin-permission-list">
                {plugin.requestedPermissions.map((permission, index) => (
                  <li key={`${permission.kind}-${index}`}>{permission.kind}</li>
                ))}
              </ul>
            )}
          </div>

          <div className="plugin-detail-section">
            <div className="plugin-detail-section-title">Runtime</div>
            <dl className="plugin-detail-meta">
              <div>
                <dt>Last healthy</dt>
                <dd>{plugin.lastHealthyAt ? new Date(plugin.lastHealthyAt).toLocaleString() : "Never"}</dd>
              </div>
              <div>
                <dt>Retry after</dt>
                <dd>{plugin.backoffUntil ? new Date(plugin.backoffUntil).toLocaleString() : "None"}</dd>
              </div>
            </dl>
          </div>

          {plugin.compatibilityReason ? (
            <div className="plugin-detail-section">
              <div className="plugin-detail-section-title">Compatibility</div>
              <p className="plugin-delete-hint">{plugin.compatibilityReason}</p>
            </div>
          ) : null}

          {plugin.lastErrorDetail ? (
            <div className="plugin-detail-section">
              <div className="plugin-detail-section-title">Last error</div>
              <pre className="plugin-detail-error">{plugin.lastErrorDetail}</pre>
            </div>
          ) : null}

          <div className="plugin-detail-actions">
            <LiquidGlassButton
              className="plugin-action plugin-action-danger"
              disabled={busy}
              onClick={onRemove}
              tone="danger"
              type="button"
            >
              <Trash2 aria-hidden="true" size={14} />
              <span>从注册表移除</span>
            </LiquidGlassButton>
          </div>
        </div>
      </details>
    </section>
  );
}
