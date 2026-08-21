import type { PluginRuntimeState } from "@megle/core-client";

const STATUS_LABEL: Record<PluginRuntimeState, string> = {
  disabled: "Disabled",
  "permission-required": "Permission required",
  starting: "Starting",
  ready: "Ready",
  degraded: "Degraded",
  "crashed-backoff": "Backoff",
  incompatible: "Incompatible",
  "configuration-required": "Configuration required"
};

interface StatusPillProps {
  status: PluginRuntimeState;
  title?: string;
}

export function StatusPill({ status, title }: StatusPillProps) {
  return (
    <span
      className={`plugin-status-pill plugin-status-pill-${status}`}
      title={title}
    >
      {STATUS_LABEL[status]}
    </span>
  );
}
