import { ArrowRightLeft, CheckCircle2, Pencil, RefreshCw, Trash2, XCircle } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { FileOperationKind, FileOperationRecord } from "@megle/core-client";
import { MediaLoadingRing } from "../loading/MediaLoadingRing";

export interface RecentOpsPanelProps {
  ops: FileOperationRecord[];
  loading: boolean;
  onRefresh: () => void;
  onDismiss?: () => void;
}

const KIND_ICON: Record<FileOperationKind, LucideIcon> = {
  rename: Pencil,
  move: ArrowRightLeft,
  delete_recycle: Trash2,
  delete_permanent: Trash2
};

const KIND_LABEL: Record<FileOperationKind, string> = {
  rename: "Rename",
  move: "Move",
  delete_recycle: "Recycle",
  delete_permanent: "Delete"
};

export function RecentOpsPanel({ ops, loading, onRefresh, onDismiss }: RecentOpsPanelProps) {
  return (
    <section aria-label="Recent file operations" className="recent-ops-panel">
      <header className="recent-ops-header">
        <div>
          <div className="panel-title">Recent file ops</div>
          <div className="panel-subtitle">{ops.length} entr{ops.length === 1 ? "y" : "ies"}</div>
        </div>
        <div className="recent-ops-actions">
          <button
            aria-label="Refresh recent operations"
            className="icon-button"
            onClick={onRefresh}
            title="Refresh"
            type="button"
          >
            <RefreshCw className={loading ? "spin" : undefined} size={14} />
          </button>
          {onDismiss ? (
            <button
              aria-label="Hide recent operations"
              className="recent-ops-dismiss"
              onClick={onDismiss}
              type="button"
            >
              Close
            </button>
          ) : null}
        </div>
      </header>

      <div className="recent-ops-list">
        {ops.length === 0 ? (
          <div
            aria-busy={loading}
            aria-label={loading ? "Refreshing recent operations" : undefined}
            className={loading ? "empty-panel empty-panel-busy" : "empty-panel"}
            role={loading ? "status" : undefined}
          >
            {loading ? <MediaLoadingRing size={18} /> : "No file operations yet"}
          </div>
        ) : (
          ops.map((op) => <RecentOpRow key={op.id} op={op} />)
        )}
      </div>
    </section>
  );
}

function RecentOpRow({ op }: { op: FileOperationRecord }) {
  const Icon = KIND_ICON[op.operation];
  const label = KIND_LABEL[op.operation];
  const sourceName = basename(op.sourcePath);
  const targetName = op.targetPath ? basename(op.targetPath) : null;
  const time = formatTime(op.createdAt);

  return (
    <div className={`recent-op-row recent-op-${op.status}`}>
      <div className="recent-op-icon">
        <Icon size={14} />
      </div>
      <div className="recent-op-body">
        <div className="recent-op-title">
          <span className="recent-op-kind">{label}</span>
          <span className="recent-op-source" title={op.sourcePath}>
            {sourceName}
          </span>
          {targetName ? (
            <>
              <span className="recent-op-arrow" aria-hidden="true">
                →
              </span>
              <span className="recent-op-target" title={op.targetPath ?? ""}>
                {targetName}
              </span>
            </>
          ) : null}
        </div>
        <div className="recent-op-meta">
          <span>{time}</span>
          <StatusPill status={op.status} />
        </div>
        {op.status === "failed" && op.error ? (
          <div className="recent-op-error" title={op.error}>
            {op.error}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function StatusPill({ status }: { status: FileOperationRecord["status"] }) {
  if (status === "succeeded") {
    return (
      <span className="recent-op-pill recent-op-pill-ok">
        <CheckCircle2 size={11} />
        <span>Done</span>
      </span>
    );
  }
  return (
    <span className="recent-op-pill recent-op-pill-fail">
      <XCircle size={11} />
      <span>Failed</span>
    </span>
  );
}

function basename(p: string): string {
  if (!p) return "";
  const normalized = p.replaceAll("\\", "/");
  const lastSlash = normalized.lastIndexOf("/");
  return lastSlash === -1 ? normalized : normalized.slice(lastSlash + 1) || normalized;
}

function formatTime(epoch: number): string {
  if (!epoch) return "";
  const ms = epoch < 1e12 ? epoch * 1000 : epoch;
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "";
  const now = Date.now();
  const delta = now - date.getTime();
  if (delta < 60_000) return "just now";
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return date.toLocaleString();
}
