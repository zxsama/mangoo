import { useEffect, useRef, useState } from "react";
import { Loader2, LogOut, RefreshCw, Settings2 } from "lucide-react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { accountKindLabel } from "./pluginAuthModel";
import { OneDriveSetupWizard } from "./OneDriveSetupWizard";
import { usePluginAuth } from "./usePluginAuth";
import type { PluginAuthController } from "./usePluginAuth";
import { deriveOneDriveSetupStep, oneDriveSetupRecoveryStep, type OneDriveSetupStep } from "./oneDriveSetupModel";

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

  const configurationEffective = Boolean(auth.configuration?.configured);
  const accountTitle = configurationEffective ? "配置已生效" : "配置未生效";
  const accountDescription = authenticated
    ? `${auth.plugin?.accountDisplayName ?? "Microsoft 账号"} · ${accountKindLabel(auth.plugin?.accountKind ?? null)}`
    : configurationStatusDescription(auth, configurationEffective);
  const openWizard = (initialStep?: OneDriveSetupStep) => {
    setWizardInitialStep(initialStep);
    setWizardOpen(true);
  };
  const configure = async () => {
    if (auth.plugin?.runtimeState === "degraded" || auth.plugin?.runtimeState === "crashed-backoff") {
      await auth.retry();
    }
    await auth.prepareConfiguration();
    openWizard(oneDriveSetupRecoveryStep(auth.error ?? auth.plugin?.lastErrorDetail ?? null) ?? deriveOneDriveSetupStep(auth.configuration, false));
  };

  return (
    <section aria-busy={unavailable} aria-label="OneDrive 账号设置" className={`onedrive-account-setup${compact ? " compact" : ""}`}>
      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? <p className="onedrive-auth-pending-note" role="status">尚未发现受信的 OneDrive Provider。</p> : null}
      <div className="onedrive-auth-actions onedrive-primary-actions">
        {auth.primaryAction !== "manage" && auth.primaryAction !== "unavailable" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void configure().catch(() => undefined)} ref={wizardTriggerRef} tone="primary" type="button"><Settings2 size={14} />配置 OneDrive</LiquidGlassButton> : null}
        {auth.primaryAction === "manage" ? <>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => openWizard(2)} ref={wizardTriggerRef} tone="control" type="button"><Settings2 size={14} />重新配置</LiquidGlassButton>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("switch")} tone="control" type="button"><RefreshCw size={14} />切换账号</LiquidGlassButton>
          <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("sign-out")} tone="danger" type="button"><LogOut size={14} />退出账号</LiquidGlassButton>
        </> : null}
        {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
      </div>
      <div className="onedrive-account-summary" role="status"><div><strong>{accountTitle}</strong><span>{accountDescription}</span></div>{authenticated ? <span className="onedrive-connected-mark">已连接</span> : null}</div>
      {confirmation ? <div className="onedrive-config-impact" role="alert"><strong>{confirmation === "switch" ? "确认切换 OneDrive 账号" : "确认退出 OneDrive 账号"}</strong><span>此操作会退出当前账号。{affectedLibraries > 0 ? `${affectedLibraries} 个同步资源库将被暂停并隔离旧会话。` : "当前没有已绑定资源库。"}</span><div><LiquidGlassButton onClick={() => setConfirmation(null)} type="button">取消</LiquidGlassButton><LiquidGlassButton onClick={() => { const action = confirmation; setConfirmation(null); if (action === "switch") void auth.signOut().then(() => openWizard(3)).catch(() => undefined); else void auth.signOut().catch(() => undefined); }} tone={confirmation === "sign-out" ? "danger" : "primary"} type="button">{confirmation === "switch" ? "退出并切换" : "暂停并退出"}</LiquidGlassButton></div></div> : null}
      <OneDriveSetupWizard affectedLibraries={affectedLibraries} auth={auth} initialStep={wizardInitialStep} onBeforeBegin={onBeforeBegin} onCancelled={onCancelled} onOpenChange={setWizardOpen} open={wizardOpen} restoreFocusRef={wizardTriggerRef} />
    </section>
  );
}

function configurationStatusDescription(auth: PluginAuthController, configurationEffective: boolean): string {
  if (!configurationEffective) {
    return auth.configuration?.readOnly ? "部署环境中的 Client ID 尚未生效。" : "完成 Entra 应用和 Client ID 配置后生效。";
  }
  if (!auth.plugin?.enabledIntent) return "插件未启用";
  if (auth.plugin.permissionRequired || auth.plugin.runtimeState === "permission-required") return "等待应用权限";
  switch (auth.primaryAction) {
    case "wait": return "正在登录";
    case "retry": return "需要重新连接";
    case "manage": return "已连接";
    default: return "尚未登录";
  }
}
