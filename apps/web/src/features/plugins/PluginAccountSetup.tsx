import { useEffect, useRef, useState } from "react";
import { Loader2, LogOut, RefreshCw, Settings2 } from "../../design/icons";
import { LiquidGlassButton } from "../../design/liquid-glass";
import type { OverlayRootId } from "../../design/overlays";
import { accountKindLabel } from "./pluginAuthModel";
import { PluginSetupWizard } from "./PluginSetupWizard";
import { usePluginAuth } from "./usePluginAuth";
import type { PluginAuthController } from "./usePluginAuth";
import { derivePluginSetupStep, type PluginSetupStep } from "./pluginSetupModel";

interface PluginAccountSetupProps {
  pluginId?: string;
  affectedLibraries?: number;
  compact?: boolean;
  overlayRootId?: OverlayRootId;
  onBeforeBegin?: () => Promise<void> | void;
  onCancelled?: () => void;
  onChanged?: () => void | Promise<void>;
  onAuthenticated?: () => void;
}

export function PluginAccountSetup(props: PluginAccountSetupProps) {
  const controller = usePluginAuth(props.pluginId, props.onChanged);
  return <PluginAccountSetupContent {...props} controller={controller} />;
}

export function PluginAccountSetupContent({
  affectedLibraries = 0,
  compact = false,
  controller: auth,
  onBeforeBegin,
  onCancelled,
  onAuthenticated,
  overlayRootId = "library",
}: PluginAccountSetupProps & { controller: PluginAuthController }) {
  const [wizardOpen, setWizardOpen] = useState(false);
  const [wizardInitialStep, setWizardInitialStep] = useState<PluginSetupStep | undefined>();
  const [confirmation, setConfirmation] = useState<"switch" | "sign-out" | null>(null);
  const wizardTriggerRef = useRef<HTMLButtonElement>(null);
  const wasAuthenticated = useRef(auth.plugin?.authState === "authenticated");
  const unavailable = auth.busy || auth.loading;
  const authenticated = auth.plugin?.authState === "authenticated";
  const presentation = auth.plugin?.contributions?.presentation;
  const accountLabel = presentation?.accountLabel ?? auth.plugin?.name ?? "账户";
  const setupLabel = presentation?.setupLabel ?? `配置 ${auth.plugin?.name ?? "插件"}`;

  useEffect(() => {
    if (authenticated && !wasAuthenticated.current && auth.remoteAvailable) onAuthenticated?.();
    wasAuthenticated.current = authenticated;
  }, [authenticated, auth.remoteAvailable, onAuthenticated]);

  const configurationEffective = Boolean(auth.configuration?.configured);
  const connectionStatus = auth.plugin?.runtimeState === "stopping"
    ? "正在停止"
    : !auth.plugin?.enabledIntent
      ? "插件已停用"
      : authenticated && auth.plugin.runtimeState === "ready"
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
    ? [accountLabel, auth.plugin?.accountDisplayName, accountKindLabel(auth.plugin?.accountKind ?? null)].filter(Boolean).join(" · ")
    : null;
  const openWizard = (initialStep?: PluginSetupStep) => {
    setWizardInitialStep(initialStep);
    setWizardOpen(true);
  };
  const configure = async () => {
    await auth.prepareConfiguration();
    openWizard(authenticated ? 2 : derivePluginSetupStep(auth.configuration, false));
  };

  return (
    <section aria-busy={unavailable} aria-label={`${accountLabel} 账号设置`} className={`plugin-account-setup${compact ? " compact" : ""}`}>
      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? <p className="plugin-auth-pending-note" role="status">尚未发现支持账户配置的插件。</p> : null}
      <div className="plugin-auth-actions plugin-primary-actions">
        {auth.primaryAction !== "unavailable" ? <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void configure().catch(() => undefined)} ref={wizardTriggerRef} tone="primary" type="button"><Settings2 size={14} />{setupLabel}</LiquidGlassButton> : null}
        {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
      </div>
      {authenticated ? <div className="plugin-auth-actions plugin-secondary-actions">
        <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => openWizard(2)} tone="control" type="button"><Settings2 size={14} />重新配置</LiquidGlassButton>
        <LiquidGlassButton className="plugin-action" disabled={unavailable || !auth.remoteAvailable} onClick={() => setConfirmation("switch")} tone="control" type="button"><RefreshCw size={14} />切换账号</LiquidGlassButton>
        <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => setConfirmation("sign-out")} tone="danger" type="button"><LogOut size={14} />退出账号</LiquidGlassButton>
      </div> : null}
      <div className="plugin-account-summary" role="status">
        <strong className="plugin-connection-status">{connectionStatus}</strong>
        {accountDescription ? <span>{accountDescription}</span> : null}
      </div>
      {confirmation ? <div className="plugin-account-confirmation" role="alert">
        <strong>{confirmation === "switch" ? `确认切换 ${accountLabel} 账号` : `确认退出 ${accountLabel} 账号`}</strong>
        <span>此操作会退出当前账号。{affectedLibraries > 0 ? `${affectedLibraries} 个资源库的插件服务将暂停。` : "相关资源库的插件服务将暂停。"}</span>
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
      <PluginSetupWizard auth={auth} initialStep={wizardInitialStep} onBeforeBegin={onBeforeBegin} onCancelled={onCancelled} onOpenChange={setWizardOpen} open={wizardOpen} restoreFocusRef={wizardTriggerRef} rootId={overlayRootId} />
    </section>
  );
}
