import { useEffect, useRef, useState } from "react";
import { Loader2, LogOut, RefreshCw, Settings2 } from "lucide-react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import type { OverlayRootId } from "../../design/overlays";
import { accountKindLabel } from "./pluginAuthModel";
import { OneDriveSetupWizard } from "./OneDriveSetupWizard";
import { usePluginAuth } from "./usePluginAuth";
import type { PluginAuthController } from "./usePluginAuth";
import { deriveOneDriveSetupStep, oneDriveSetupRecoveryStep, type OneDriveSetupStep } from "./oneDriveSetupModel";

interface OneDriveAccountSetupProps {
  affectedLibraries?: number;
  compact?: boolean;
  overlayRootId?: OverlayRootId;
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
  overlayRootId = "library",
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
  const connectionStatus = authenticated
    ? "已连接"
    : !configurationEffective
      ? "未配置"
      : !auth.plugin?.enabledIntent
        ? "插件未启用"
        : auth.plugin.permissionRequired || auth.plugin.runtimeState === "permission-required"
          ? "等待应用权限"
          : auth.primaryAction === "wait"
            ? "正在登录"
            : auth.primaryAction === "retry"
              ? "需要重新连接"
              : "未连接";
  const accountDescription = authenticated
    ? ["Microsoft 账号", auth.plugin?.accountDisplayName, accountKindLabel(auth.plugin?.accountKind ?? null)].filter(Boolean).join(" · ")
    : null;
  const openWizard = (initialStep?: OneDriveSetupStep) => {
    setWizardInitialStep(initialStep);
    setWizardOpen(true);
  };
  const configure = async () => {
    if (auth.plugin?.runtimeState === "degraded" || auth.plugin?.runtimeState === "crashed-backoff") {
      await auth.retry();
    }
    await auth.prepareConfiguration();
    openWizard(oneDriveSetupRecoveryStep(auth.error ?? auth.plugin?.lastErrorDetail ?? null) ?? (authenticated ? 2 : deriveOneDriveSetupStep(auth.configuration, false)));
  };

  return (
    <section aria-busy={unavailable} aria-label="OneDrive 账号设置" className={`onedrive-account-setup${compact ? " compact" : ""}`}>
      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? <p className="onedrive-auth-pending-note" role="status">尚未发现受信的 OneDrive Provider。</p> : null}
      <div className="onedrive-auth-actions onedrive-primary-actions">
        {auth.primaryAction !== "unavailable" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void configure().catch(() => undefined)} ref={wizardTriggerRef} tone="primary" type="button"><Settings2 size={14} />配置 OneDrive</LiquidGlassButton> : null}
        {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
      </div>
      {authenticated ? <div className="onedrive-auth-actions onedrive-secondary-actions">
        <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => openWizard(2)} tone="control" type="button"><Settings2 size={14} />重新配置</LiquidGlassButton>
        <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => setConfirmation("switch")} tone="control" type="button"><RefreshCw size={14} />切换账号</LiquidGlassButton>
        <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => setConfirmation("sign-out")} tone="danger" type="button"><LogOut size={14} />退出账号</LiquidGlassButton>
      </div> : null}
      <div className="onedrive-account-summary" role="status">
        <strong className="onedrive-connection-status">{connectionStatus}</strong>
        {accountDescription ? <span>{accountDescription}</span> : null}
      </div>
      {confirmation ? <div className="onedrive-account-confirmation" role="alert">
        <strong>{confirmation === "switch" ? "确认切换 OneDrive 账号" : "确认退出 OneDrive 账号"}</strong>
        <span>此操作会退出当前账号。{affectedLibraries > 0 ? `${affectedLibraries} 个同步资源库将暂停同步。` : "相关同步资源库将暂停同步。"}</span>
        <div>
          <LiquidGlassButton disabled={auth.busy} onClick={() => setConfirmation(null)} type="button">取消</LiquidGlassButton>
          <LiquidGlassButton disabled={auth.busy} onClick={() => {
            const action = confirmation;
            setConfirmation(null);
            void auth.signOut().then(() => {
              if (action === "switch") openWizard(3);
            }).catch(() => undefined);
          }} tone={confirmation === "sign-out" ? "danger" : "primary"} type="button">{confirmation === "switch" ? "退出并切换" : "退出账号"}</LiquidGlassButton>
        </div>
      </div> : null}
      <OneDriveSetupWizard auth={auth} initialStep={wizardInitialStep} onBeforeBegin={onBeforeBegin} onCancelled={onCancelled} onOpenChange={setWizardOpen} open={wizardOpen} restoreFocusRef={wizardTriggerRef} rootId={overlayRootId} />
    </section>
  );
}
