import { Database, Trash2 } from "lucide-react";
import type { HostPluginProjection, PluginCapability } from "@megle/core-client";
import { StatusPill } from "./StatusPill";
import { OneDriveAccountSetup } from "./OneDriveAccountSetup";
import { ONEDRIVE_PLUGIN_ID } from "./pluginAuthModel";

interface PluginDetailProps {
  plugin: HostPluginProjection;
  busy: boolean;
  onApprovePermissions: () => void;
  onRetry: () => void;
  onRemove: () => void;
  onRefresh: () => void | Promise<void>;
}

const CAPABILITY_LABEL: Record<PluginCapability, string> = {
  "sync-provider.v1": "Sync provider v1"
};

export function PluginDetail({
  plugin,
  busy,
  onApprovePermissions,
  onRetry,
  onRemove,
  onRefresh
}: PluginDetailProps) {
  return (
    <section className="plugin-detail" aria-label={`${plugin.pluginId} details`}>
      <header className="plugin-detail-header">
        <div className="plugin-detail-heading">
          <div className="plugin-detail-titles">
            <div className="plugin-detail-title">{plugin.pluginId}</div>
            <div className="plugin-detail-subtitle">
              <span>v{plugin.selectedVersion}</span>
              <span className="plugin-detail-id" title={plugin.pluginId}>
                {plugin.pluginId}
              </span>
            </div>
          </div>
          <StatusPill status={plugin.runtimeState} title={plugin.lastErrorDetail ?? undefined} />
        </div>
      </header>

      <div className="plugin-detail-section">
        <div className="plugin-detail-section-title">Capabilities</div>
        {plugin.capabilities.length === 0 ? (
          <div className="plugin-detail-empty">None declared.</div>
        ) : (
          <div className="plugin-capability-chips">
            {plugin.capabilities.map((capability) => {
              return (
                <span className="plugin-capability-chip" key={capability}>
                  <Database size={12} aria-hidden="true" />
                  <span>{CAPABILITY_LABEL[capability]}</span>
                </span>
              );
            })}
          </div>
        )}
      </div>

      {plugin.pluginId === ONEDRIVE_PLUGIN_ID ? (
        <div className="plugin-detail-section">
          <div className="plugin-detail-section-title">OneDrive account</div>
          <OneDriveAccountSetup onChanged={onRefresh} />
        </div>
      ) : null}

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
            <dt>Intent</dt>
            <dd>{plugin.enabledIntent ? "Enabled" : "Disabled"}</dd>
          </div>
          <div>
            <dt>Authentication</dt>
            <dd>{plugin.accountDisplayName ?? plugin.authState ?? "No account"}</dd>
          </div>
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
        {plugin.permissionRequired ? (
          <button className="plugins-rescan" onClick={onApprovePermissions} type="button" disabled={busy}>
            Approve requested permissions
          </button>
        ) : null}
        {plugin.runtimeState === "crashed-backoff" || plugin.runtimeState === "degraded" ? (
          <button className="plugins-rescan" onClick={onRetry} type="button" disabled={busy}>
            Retry provider
          </button>
        ) : null}
        <button
          className="plugin-delete-button"
          onClick={onRemove}
          type="button"
          disabled={busy}
        >
          <Trash2 size={14} />
          <span>Remove from registry</span>
        </button>
        <p className="plugin-delete-hint">
          Removal is blocked while an account or Library binding references this provider.
        </p>
      </div>
    </section>
  );
}
