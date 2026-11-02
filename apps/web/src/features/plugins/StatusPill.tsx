import type { PluginRuntimeState } from "@mangoo/core-client";

const STATUS_LABEL: Record<PluginRuntimeState, string> = {
  disabled: "Disabled",
  stopping: "Stopping",
  "permission-required": "Permission required",
  starting: "Starting",
  ready: "Enable",
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
