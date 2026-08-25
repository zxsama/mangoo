import { useEffect, useRef, useState } from "react";
import { Check, Clipboard, ExternalLink, Loader2, LogOut, RefreshCw, X } from "lucide-react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  accountKindLabel,
  configurationSourceLabel,
  formatAuthRemaining,
} from "./pluginAuthModel";
import { usePluginAuth } from "./usePluginAuth";
import type { PluginAuthController } from "./usePluginAuth";

interface OneDriveAccountSetupProps {
  affectedLibraries?: number;
  compact?: boolean;
  onBeforeBegin?: () => Promise<void> | void;
  onCancelled?: () => void;
  onChanged?: () => void | Promise<void>;
  onAuthenticated?: () => void;
}

export function OneDriveAccountSetup(props: OneDriveAccountSetupProps) {
  const controller = usePluginAuth(undefined, props.onChanged);
  return <OneDriveAccountSetupContent {...props} controller={controller} />;
}

export function OneDriveAccountSetupContent({ affectedLibraries = 0, compact = false, controller: auth, onBeforeBegin, onCancelled, onAuthenticated }: OneDriveAccountSetupProps & { controller: PluginAuthController }) {
  const [clientId, setClientId] = useState("");
  const [editingConfiguration, setEditingConfiguration] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [confirmation, setConfirmation] = useState<"switch" | "sign-out" | null>(null);
  const wasAuthenticated = useRef(false);
  const unavailable = auth.busy || auth.loading;

  useEffect(() => {
    if (!auth.flow) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [auth.flow]);

  useEffect(() => {
    const authenticated = auth.plugin?.authState === "authenticated";
    if (authenticated && !wasAuthenticated.current) onAuthenticated?.();
    wasAuthenticated.current = authenticated;
  }, [auth.plugin?.authState, onAuthenticated]);

  const accountLabel = auth.plugin?.accountDisplayName ?? "尚未连接账号";
  const accountKind = accountKindLabel(auth.plugin?.accountKind ?? null);

  async function submitConfiguration() {
    const normalized = clientId.trim();
    await auth.planConfiguration(normalized || null).catch(() => undefined);
  }

  return (
    <section className={`onedrive-account-setup${compact ? " compact" : ""}`} aria-busy={auth.busy || auth.loading} aria-label="OneDrive 账号设置">
      <div className="onedrive-account-summary">
        <div>
          <strong>{accountLabel}</strong>
          <span>{auth.plugin?.authState === "authenticated" ? accountKind : "OneDrive 尚未连接"}</span>
        </div>
        <span className={`onedrive-auth-state ${auth.plugin?.authState ?? "signed-out"}`} role="status">
          {auth.plugin?.authState === "authenticated" ? "已连接" : auth.plugin?.authState === "authorizing" ? "正在授权" : "未连接"}
        </span>
      </div>

      {auth.configuration ? (
        <div className="onedrive-config-summary">
          <span>应用配置：{configurationSourceLabel(auth.configuration.source)}</span>
          {auth.configuration.errorCode ? <small role="alert">{auth.configuration.errorCode}</small> : null}
          {!auth.configuration.readOnly ? (
            <button data-interactive-pointer-target="true" disabled={auth.busy} onClick={() => setEditingConfiguration((value) => !value)} type="button">
              {editingConfiguration ? "收起高级配置" : "高级配置"}
            </button>
          ) : null}
        </div>
      ) : null}

      {editingConfiguration && auth.configuration && !auth.configuration.readOnly ? (
        <div className="onedrive-config-editor">
          <label>
            <span>自定义 Microsoft Entra Client ID</span>
            <input autoComplete="off" disabled={auth.busy} onChange={(event) => setClientId(event.target.value)} placeholder="00000000-0000-0000-0000-000000000000" spellCheck={false} value={clientId} />
          </label>
          <small>留空会清除此设备覆盖并恢复内置配置。Client ID 不是密码，但更改会退出当前账号并暂停受影响资源库。</small>
          <button data-interactive-pointer-target="true" disabled={auth.busy} onClick={() => void submitConfiguration()} type="button">预览更改</button>
        </div>
      ) : null}

      {auth.configurationPlan ? (
        <div className="onedrive-config-impact" role="alert">
          <strong>{auth.configurationPlan.effectiveChanged ? "配置将发生变化" : "有效配置没有变化"}</strong>
          <span>{auth.configurationPlan.effectiveChanged ? `${auth.configurationPlan.affectedLibraries} 个资源库会被暂停并隔离旧账号会话。` : "提交不会退出账号或改变资源库状态。"}</span>
          <div>
            <button data-interactive-pointer-target="true" onClick={auth.clearConfigurationPlan} type="button">取消</button>
            <LiquidGlassButton disabled={auth.busy} onClick={() => void auth.commitConfiguration(auth.configurationPlan?.requiresConfirmation ?? false).catch(() => undefined)} type="button">确认应用</LiquidGlassButton>
          </div>
        </div>
      ) : null}

      {auth.flow ? (
        <div className="onedrive-auth-flow" aria-live="polite">
          <div>
            <strong>在 Microsoft 页面完成授权</strong>
            <span>剩余 {formatAuthRemaining(auth.flow.expiresAt, now)}</span>
          </div>
          <div className="onedrive-auth-actions">
            <button data-interactive-pointer-target="true" onClick={() => void auth.copyAuthorizationUrl()} type="button"><Clipboard aria-hidden="true" size={14} />复制授权链接</button>
            <button data-interactive-pointer-target="true" onClick={() => void auth.openAuthorizationUrl()} type="button"><ExternalLink aria-hidden="true" size={14} />重新打开</button>
            <button data-interactive-pointer-target="true" disabled={auth.busy} onClick={() => { onCancelled?.(); void auth.cancel().catch(() => undefined); }} type="button"><X aria-hidden="true" size={14} />取消授权</button>
          </div>
          {auth.flow.completionMode === "manual-response-uri" ? (
            <form className="onedrive-manual-completion" onSubmit={(event) => { event.preventDefault(); void auth.complete().catch(() => undefined); }}>
              <label>
                <span>完整 response URI</span>
                <textarea autoComplete="off" disabled={auth.busy} onChange={(event) => auth.setResponseUri(event.target.value)} placeholder="完成登录后，复制浏览器地址栏中的完整地址并粘贴到这里" rows={3} spellCheck={false} value={auth.responseUri} />
              </label>
              <LiquidGlassButton disabled={auth.busy || !auth.responseUri.trim()} type="submit"><Check aria-hidden="true" size={14} />完成连接</LiquidGlassButton>
            </form>
          ) : <small>此窗口会在授权完成后自动更新，可以安全地保持打开。</small>}
        </div>
      ) : null}

      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? <p className="onedrive-auth-pending-note" role="status">尚未发现受信的 OneDrive Provider，请先在插件面板重新扫描。</p> : null}

      {!auth.flow ? (
        <div className="onedrive-auth-actions">
          {auth.primaryAction === "enable" ? <LiquidGlassButton disabled={unavailable} onClick={() => void auth.enable().catch(() => undefined)} type="button">启用 OneDrive</LiquidGlassButton> : null}
          {auth.primaryAction === "approve" ? <LiquidGlassButton disabled={unavailable} onClick={() => void auth.approve().catch(() => undefined)} type="button">批准所需权限</LiquidGlassButton> : null}
          {auth.primaryAction === "configure" ? <LiquidGlassButton disabled={unavailable || auth.configuration?.readOnly} onClick={() => setEditingConfiguration(true)} type="button">配置 Client ID</LiquidGlassButton> : null}
          {auth.primaryAction === "connect" || auth.primaryAction === "retry" ? <LiquidGlassButton disabled={unavailable} onClick={() => void (auth.primaryAction === "retry" && (auth.plugin?.runtimeState === "degraded" || auth.plugin?.runtimeState === "crashed-backoff") ? auth.retry() : Promise.resolve(onBeforeBegin?.()).then(() => auth.begin())).catch(() => undefined)} type="button">{auth.primaryAction === "retry" ? <RefreshCw aria-hidden="true" size={14} /> : <ExternalLink aria-hidden="true" size={14} />}{auth.primaryAction === "retry" ? "重试 OneDrive" : "连接 OneDrive"}</LiquidGlassButton> : null}
          {auth.primaryAction === "manage" ? <>
            <button data-interactive-pointer-target="true" disabled={auth.busy} onClick={() => setConfirmation("switch")} type="button"><RefreshCw aria-hidden="true" size={14} />切换账号</button>
            <button data-interactive-pointer-target="true" disabled={auth.busy} onClick={() => setConfirmation("sign-out")} type="button"><LogOut aria-hidden="true" size={14} />退出账号</button>
          </> : null}
          {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
        </div>
      ) : null}
      {!auth.flow && auth.plugin?.authState === "authorizing" ? <p className="onedrive-auth-pending-note" role="status">授权正在另一个界面中进行；请回到发起授权的窗口完成或取消。</p> : null}
      {confirmation ? (
        <div className="onedrive-config-impact" role="alert">
          <strong>{confirmation === "switch" ? "确认切换 OneDrive 账号" : "确认退出 OneDrive 账号"}</strong>
          <span>此操作会退出当前账号。{affectedLibraries > 0 ? `${affectedLibraries} 个同步资源库将被暂停并隔离旧会话。` : "当前没有已绑定资源库。"}{confirmation === "switch" ? "随后会立即开始新的授权。" : ""}</span>
          <div>
            <button data-interactive-pointer-target="true" onClick={() => setConfirmation(null)} type="button">取消</button>
            <LiquidGlassButton onClick={() => { const action = confirmation; setConfirmation(null); if (action === "switch") void Promise.resolve(onBeforeBegin?.()).then(() => auth.begin(true)).catch(() => undefined); else void auth.signOut().catch(() => undefined); }} type="button">
              {confirmation === "switch" ? "暂停并切换" : "暂停并退出"}
            </LiquidGlassButton>
          </div>
        </div>
      ) : null}
    </section>
  );
}
