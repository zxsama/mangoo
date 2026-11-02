import { CoreApiError } from "../../core/client.ts";

const libraryErrorMessages: Record<string, string> = {
  "invalid-path": "位置无效，请选择已有 Mangoo 资源库文件夹。",
  "library-path-not-found": "找不到此文件夹，请检查位置或重新选择资源库。",
  "library-path-unavailable-or-reparse": "无法访问此位置，请选择实际文件夹，并检查磁盘连接和访问权限。",
  "library-marker-not-found": "此文件夹不是 Mangoo 资源库，请选择已有资源库的 Database 目录。",
  "invalid-library-marker": "此文件夹的资源库信息无效，请选择完整的 Mangoo 资源库。",
  "library-device-identity-mismatch": "此资源库来自另一设备，暂不支持直接导入。原目录保持不变。",
  "unsupported-library-marker": "此资源库的格式版本暂不受支持，请使用兼容版本的 Mangoo 打开。",
  "library-marker-not-committed": "此资源库尚未完成创建或恢复，请完成原操作后重试。",
  "library-identity-conflict": "此资源库的身份与已登记的位置不一致，请选择原资源库位置。",
  "marker-mismatch": "此文件夹与登记的资源库不一致，请重新选择原资源库位置。",
  "marker-identity-mismatch": "资源库信息已变化，请重新选择原资源库位置。",
  "library-local-data-conflict": "本机已有不同的资源库数据，请检查原有资源库记录后重试。",
  "library-local-data-unavailable": "此资源库的本机目录数据无法读取，请检查磁盘连接和访问权限后重试。",
  "nested-library-root": "此位置与已有资源库或应用数据目录重叠，请选择独立的资源库文件夹。",
  "library-open-conflict": "此位置正在由另一项打开操作使用，请稍后重试。",
  "library-not-found": "资源库已不在列表中，请刷新列表或通过“打开其它资源库”重新打开。",
  "library-unavailable": "资源库暂不可用，请检查存储位置并在当前操作完成后重试。",
  "path-changed": "资源库位置已变化，请重新选择文件夹后重试。",
  "stale-context-revision": "活动资源库已改变，请确认当前资源库后重试。",
  "invalid-library-name": "资源库文件夹名称无效，请选择有效的资源库位置。",
  "host-shutting-down": "应用正在关闭，请重新启动后重试。",
  "library-stop-failed": "暂时无法关闭当前资源库，请等待当前操作完成后重试。",
};

export function libraryOperationError(cause: unknown): string {
  if (cause instanceof CoreApiError) {
    if (cause.body && typeof cause.body === "object") {
      const body = cause.body as Record<string, unknown>;
      // Core's ErrorResponse is { error, code }; message remains a compatibility
      // fallback for older adapters. Never turn an unknown cause into success.
      const message = typeof body.error === "string" ? body.error : body.message;
      const rawCode = typeof body.code === "string" ? body.code : message;
      const code = typeof rawCode === "string" ? rawCode.split(":", 1)[0].trim().replaceAll("_", "-") : "";
      if (libraryErrorMessages[code]) return libraryErrorMessages[code];
      if (typeof message === "string" && message.trim()) return message;
    }
    if (cause.status === 404) return "未找到资源库或所选位置，请重新选择已有 Mangoo 资源库文件夹。";
  }
  if (cause instanceof TypeError && ["Failed to fetch", "Load failed", "NetworkError when attempting to fetch resource."].includes(cause.message)) {
    return "无法连接资源库服务，请检查连接后重试。";
  }
  if (cause instanceof Error && cause.message) return cause.message;
  return typeof cause === "string" ? cause : String(cause);
}
