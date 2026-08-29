import { useEffect, useRef, useState } from "react";
import { ExternalLink, Loader2, LogOut, RefreshCw, Settings2 } from "lucide-react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { accountKindLabel } from "./pluginAuthModel";
import { OneDriveSetupWizard } from "./OneDriveSetupWizard";
import { usePluginAuth } from "./usePluginAuth";
import type { PluginAuthController } from "./usePluginAuth";
import type { OneDriveSetupStep } from "./oneDriveSetupModel";

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

export function OneDriveAccountSetupContent({
  affectedLibraries = 0,
  compact = false,
  controller: auth,
  onBeforeBegin,
  onCancelled,
  onAuthenticated,
}: OneDriveAccountSetupProps & { controller: PluginAuthController }) {
  const [wizardOpen, setWizardOpen] = useState(false);
  const [wizardInitialStep, setWizardInitialStep] = useState<OneDriveSetupStep | undefined>();
  const [confirmation, setConfirmation] = useState<"switch" | "sign-out" | null>(null);
  const wizardTriggerRef = useRef<HTMLButtonElement>(null);
  const wasAuthenticated = useRef(auth.plugin?.authState === "authenticated");
  const unavailable = auth.busy || auth.loading;
  const authenticated = auth.plugin?.authState === "authenticated";

  useEffect(() => {
    if (authenticated && !wasAuthenticated.current) onAuthenticated?.();
    wasAuthenticated.current = authenticated;
  }, [authenticated, onAuthenticated]);

  const accountTitle = authenticated ? auth.plugin?.accountDisplayName ?? "Microsoft 账号" : primaryStateTitle(auth.primaryAction);
  const accountDescription = authenticated ? accountKindLabel(auth.plugin?.accountKind ?? null) : primaryStateDescription(auth);
  const openWizard = (initialStep?: OneDriveSetupStep) => {
    setWizardInitialStep(initialStep);
    setWizardOpen(true);
  };

  return (
    <section aria-busy={unavailable} aria-label="OneDrive 账号设置" className={`onedrive-account-setup${compact ? " compact" : ""}`}>
      <div className="onedrive-account-summary" role="status"><div><strong>{accountTitle}</strong><span>{accountDescription}</span></div>{authenticated ? <span className="onedrive-connected-mark">已连接</span> : null}</div>
      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? <p className="onedrive-auth-pending-note" role="status">尚未发现受信的 OneDrive Provider。</p> : null}
      <div className="onedrive-auth-actions onedrive-primary-actions">
        {auth.primaryAction === "enable" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void auth.enable().catch(() => undefined)} tone="primary" type="button">启用 OneDrive</LiquidGlassButton> : null}
        {auth.primaryAction === "approve" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void auth.approve().catch(() => undefined)} tone="primary" type="button">批准所需权限</LiquidGlassButton> : null}
        {auth.primaryAction === "configure" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => openWizard(1)} ref={wizardTriggerRef} tone="primary" type="button"><Settings2 size={14} />配置 OneDrive</LiquidGlassButton> : null}
        {auth.primaryAction === "connect" || auth.primaryAction === "wait" ? <LiquidGlassButton className="plugin-action" disabled={unavailable && auth.primaryAction !== "wait"} onClick={() => openWizard(4)} ref={wizardTriggerRef} tone="primary" type="button"><ExternalLink size={14} />{auth.primaryAction === "wait" ? "继续登录" : "登录 Microsoft 账号"}</LiquidGlassButton> : null}
        {auth.primaryAction === "retry" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => auth.plugin?.runtimeState === "degraded" || auth.plugin?.runtimeState === "crashed-backoff" ? void auth.retry().catch(() => undefined) : openWizard(4)} ref={wizardTriggerRef} tone="primary" type="button"><RefreshCw size={14} />重新连接</LiquidGlassButton> : null}
        {auth.primaryAction === "manage" ? <>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => openWizard(3)} ref={wizardTriggerRef} tone="control" type="button"><Settings2 size={14} />重新配置</LiquidGlassButton>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("switch")} tone="control" type="button"><RefreshCw size={14} />切换账号</LiquidGlassButton>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("sign-out")} tone="danger" type="button"><LogOut size={14} />退出账号</LiquidGlassButton>
        </> : null}
        {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
      </div>
      {confirmation ? <div className="onedrive-config-impact" role="alert"><strong>{confirmation === "switch" ? "确认切换 OneDrive 账号" : "确认退出 OneDrive 账号"}</strong><span>此操作会退出当前账号。{affectedLibraries > 0 ? `${affectedLibraries} 个同步资源库将被暂停并隔离旧会话。` : "当前没有已绑定资源库。"}</span><div><LiquidGlassButton onClick={() => setConfirmation(null)} type="button">取消</LiquidGlassButton><LiquidGlassButton onClick={() => { const action = confirmation; setConfirmation(null); if (action === "switch") void auth.signOut().then(() => openWizard(4)).catch(() => undefined); else void auth.signOut().catch(() => undefined); }} tone={confirmation === "sign-out" ? "danger" : "primary"} type="button">{confirmation === "switch" ? "退出并切换" : "暂停并退出"}</LiquidGlassButton></div></div> : null}
      <OneDriveSetupWizard affectedLibraries={affectedLibraries} auth={auth} initialStep={wizardInitialStep} onBeforeBegin={onBeforeBegin} onCancelled={onCancelled} onOpenChange={setWizardOpen} open={wizardOpen} restoreFocusRef={wizardTriggerRef} />
    </section>
  );
}

function primaryStateTitle(action: PluginAuthController["primaryAction"]): string {
  switch (action) {
    case "enable": return "OneDrive 已停用";
    case "approve": return "需要批准权限";
    case "configure": return "尚未配置 OneDrive";
    case "connect": return "尚未登录 Microsoft 账号";
    case "wait": return "正在登录 Microsoft 账号";
    case "retry": return "需要重新连接";
    case "manage": return "OneDrive 已连接";
    default: return "OneDrive Provider 不可用";
  }
}

function primaryStateDescription(auth: PluginAuthController): string {
  switch (auth.primaryAction) {
    case "enable": return "启用插件后即可开始配置。";
    case "approve": return "先批准 Provider 请求的本机权限。";
    case "configure": return auth.configuration?.readOnly ? "部署环境中的 Client ID 尚未生效。" : "通过五个步骤创建并连接你自己的 Entra 应用。";
    case "connect": return "使用已配置的自建应用登录 Personal 或 Business 账号。";
    case "wait": return "请继续完成 Microsoft 授权。";
    case "retry": return "检查自建应用配置后重新登录。";
    case "manage": return accountKindLabel(auth.plugin?.accountKind ?? null);
    default: return "正在自动检测已安装的插件。";
  }
}
