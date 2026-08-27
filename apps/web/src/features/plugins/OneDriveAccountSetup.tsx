import { useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronRight,
  Clipboard,
  ExternalLink,
  Loader2,
  LogOut,
  RefreshCw,
  X
} from "lucide-react";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  accountKindLabel,
  configurationSourceLabel,
  formatAuthRemaining
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

export function OneDriveAccountSetupContent({
  affectedLibraries = 0,
  compact = false,
  controller: auth,
  onBeforeBegin,
  onCancelled,
  onAuthenticated
}: OneDriveAccountSetupProps & { controller: PluginAuthController }) {
  const [clientId, setClientId] = useState("");
  const [editingConfiguration, setEditingConfiguration] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [confirmation, setConfirmation] = useState<"switch" | "sign-out" | null>(null);
  const configurationInputRef = useRef<HTMLInputElement | null>(null);
  const focusConfigurationInputOnOpenRef = useRef(false);
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

  useEffect(() => {
    if (!editingConfiguration || auth.configuration?.readOnly || !focusConfigurationInputOnOpenRef.current) return;
    const frame = window.requestAnimationFrame(() => {
      configurationInputRef.current?.focus();
      focusConfigurationInputOnOpenRef.current = false;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [auth.configuration?.readOnly, editingConfiguration]);

  const authenticated = auth.plugin?.authState === "authenticated";
  const accountTitle = authenticated
    ? auth.plugin?.accountDisplayName ?? "OneDrive"
    : primaryStateTitle(auth.primaryAction);
  const accountDescription = authenticated
    ? accountKindLabel(auth.plugin?.accountKind ?? null)
    : primaryStateDescription(auth);

  async function submitConfiguration() {
    const normalized = clientId.trim();
    await auth.planConfiguration(normalized || null).catch(() => undefined);
  }

  function openConfiguration() {
    focusConfigurationInputOnOpenRef.current = true;
    if (editingConfiguration) {
      configurationInputRef.current?.focus();
      focusConfigurationInputOnOpenRef.current = false;
      return;
    }
    setEditingConfiguration(true);
  }

  return (
    <section
      aria-busy={auth.busy || auth.loading}
      aria-label="OneDrive 账号设置"
      className={`onedrive-account-setup${compact ? " compact" : ""}`}
    >
      <div className="onedrive-account-summary" role="status">
        <strong>{accountTitle}</strong>
        <span>{accountDescription}</span>
      </div>

      {auth.flow ? (
        <div className="onedrive-auth-flow" aria-live="polite">
          <div>
            <strong>在 Microsoft 页面完成授权</strong>
            <span>剩余 {formatAuthRemaining(auth.flow.expiresAt, now)}</span>
          </div>
          <div className="onedrive-auth-actions">
            <LiquidGlassButton className="plugin-action" onClick={() => void auth.copyAuthorizationUrl()} tone="control" type="button">
              <Clipboard aria-hidden="true" size={14} />
              复制授权链接
            </LiquidGlassButton>
            <LiquidGlassButton className="plugin-action" onClick={() => void auth.openAuthorizationUrl()} tone="control" type="button">
              <ExternalLink aria-hidden="true" size={14} />
              重新打开
            </LiquidGlassButton>
            <LiquidGlassButton
              className="plugin-action"
              disabled={auth.busy}
              onClick={() => {
                void auth.cancel().then(() => onCancelled?.()).catch(() => undefined);
              }}
              tone="control"
              type="button"
            >
              <X aria-hidden="true" size={14} />
              取消授权
            </LiquidGlassButton>
          </div>
          {auth.flow.completionMode === "manual-response-uri" ? (
            <form
              className="onedrive-manual-completion"
              onSubmit={(event) => {
                event.preventDefault();
                void auth.complete().catch(() => undefined);
              }}
            >
              <label>
                <span>完整 response URI</span>
                <textarea
                  autoComplete="off"
                  disabled={auth.busy}
                  onChange={(event) => auth.setResponseUri(event.target.value)}
                  placeholder="完成登录后，复制浏览器地址栏中的完整地址并粘贴到这里"
                  rows={3}
                  spellCheck={false}
                  value={auth.responseUri}
                />
              </label>
              <LiquidGlassButton className="plugin-action" disabled={auth.busy || !auth.responseUri.trim()} tone="primary" type="submit">
                <Check aria-hidden="true" size={14} />
                完成连接
              </LiquidGlassButton>
            </form>
          ) : (
            <small>授权完成后此窗口会自动更新，可以保持打开。</small>
          )}
        </div>
      ) : null}

      {auth.error ? <div className="settings-inline-error" role="alert">{auth.error}</div> : null}
      {auth.primaryAction === "unavailable" && !auth.loading ? (
        <p className="onedrive-auth-pending-note" role="status">
          尚未发现受信的 OneDrive Provider。
        </p>
      ) : null}

      {!auth.flow ? (
        <div className="onedrive-auth-actions onedrive-primary-actions">
          {auth.primaryAction === "enable" ? (
            <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void auth.enable().catch(() => undefined)} tone="primary" type="button">
              启用 OneDrive
            </LiquidGlassButton>
          ) : null}
          {auth.primaryAction === "approve" ? (
            <LiquidGlassButton className="plugin-action" disabled={unavailable} onClick={() => void auth.approve().catch(() => undefined)} tone="primary" type="button">
              批准所需权限
            </LiquidGlassButton>
          ) : null}
          {auth.primaryAction === "configure" ? (
            <LiquidGlassButton
              className="plugin-action"
              disabled={unavailable || auth.configuration?.readOnly}
              onClick={openConfiguration}
              tone="primary"
              type="button"
            >
              配置 Client ID
            </LiquidGlassButton>
          ) : null}
          {auth.primaryAction === "connect" || auth.primaryAction === "retry" ? (
            <LiquidGlassButton
              className="plugin-action"
              disabled={unavailable}
              onClick={() => void (
                auth.primaryAction === "retry" &&
                (auth.plugin?.runtimeState === "degraded" || auth.plugin?.runtimeState === "crashed-backoff")
                  ? auth.retry()
                  : Promise.resolve(onBeforeBegin?.()).then(() => auth.begin())
              ).catch(() => undefined)}
              tone="primary"
              type="button"
            >
              {auth.primaryAction === "retry" ? <RefreshCw aria-hidden="true" size={14} /> : <ExternalLink aria-hidden="true" size={14} />}
              {auth.primaryAction === "retry" ? "重试 OneDrive" : "连接 OneDrive"}
            </LiquidGlassButton>
          ) : null}
          {auth.primaryAction === "manage" ? (
            <>
              <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("switch")} tone="control" type="button">
                <RefreshCw aria-hidden="true" size={14} />
                切换账号
              </LiquidGlassButton>
              <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => setConfirmation("sign-out")} tone="danger" type="button">
                <LogOut aria-hidden="true" size={14} />
                退出账号
              </LiquidGlassButton>
            </>
          ) : null}
          {auth.busy ? <Loader2 aria-label="正在处理" className="spin" size={16} /> : null}
        </div>
      ) : null}

      {!auth.flow && auth.plugin?.authState === "authorizing" ? (
        <p className="onedrive-auth-pending-note" role="status">
          授权正在另一个界面中进行，请回到发起授权的窗口完成或取消。
        </p>
      ) : null}

      {confirmation ? (
        <div className="onedrive-config-impact" role="alert">
          <strong>{confirmation === "switch" ? "确认切换 OneDrive 账号" : "确认退出 OneDrive 账号"}</strong>
          <span>
            此操作会退出当前账号。
            {affectedLibraries > 0 ? `${affectedLibraries} 个同步资源库将被暂停并隔离旧会话。` : "当前没有已绑定资源库。"}
            {confirmation === "switch" ? "随后会立即开始新的授权。" : ""}
          </span>
          <div>
            <LiquidGlassButton className="plugin-action" onClick={() => setConfirmation(null)} tone="control" type="button">
              取消
            </LiquidGlassButton>
            <LiquidGlassButton
              className="plugin-action"
              onClick={() => {
                const action = confirmation;
                setConfirmation(null);
                if (action === "switch") {
                  void Promise.resolve(onBeforeBegin?.()).then(() => auth.begin(true)).catch(() => onCancelled?.());
                } else {
                  void auth.signOut().catch(() => undefined);
                }
              }}
              tone={confirmation === "sign-out" ? "danger" : "primary"}
              type="button"
            >
              {confirmation === "switch" ? "暂停并切换" : "暂停并退出"}
            </LiquidGlassButton>
          </div>
        </div>
      ) : null}

      {auth.configuration ? (
        <details
          className="plugin-disclosure onedrive-advanced-configuration"
          onToggle={(event) => setEditingConfiguration(event.currentTarget.open)}
          open={editingConfiguration}
        >
          <summary className="plugin-disclosure-summary" data-pointer-glow="off">
            <ChevronRight aria-hidden="true" className="plugin-disclosure-chevron" size={15} />
            <span className="plugin-disclosure-label">高级配置</span>
          </summary>
          <div className="plugin-disclosure-body">
            <p className="onedrive-advanced-copy">
              配置来源：{configurationSourceLabel(auth.configuration.source)}
            </p>
            {auth.configuration.readOnly ? (
              <p className="onedrive-advanced-copy">
                请为部署设置有效的 MEGLE_ONEDRIVE_CLIENT_ID，然后重启 Megle。环境配置不能在界面中覆盖。
              </p>
            ) : (
              <div className="onedrive-config-editor">
                <label>
                  <span>自定义 Microsoft Entra Client ID</span>
                  <input
                    autoComplete="off"
                    disabled={auth.busy}
                    onChange={(event) => setClientId(event.target.value)}
                    placeholder="00000000-0000-0000-0000-000000000000"
                    ref={configurationInputRef}
                    spellCheck={false}
                    value={clientId}
                  />
                </label>
                <small>
                  留空会清除此设备覆盖并恢复内置配置，不需要 client secret。更改会退出当前账号并暂停受影响资源库。
                </small>
                <LiquidGlassButton className="plugin-action" disabled={auth.busy} onClick={() => void submitConfiguration()} tone="control" type="button">
                  检查更改
                </LiquidGlassButton>
              </div>
            )}

            {auth.configuration.errorCode ? (
              <p className="onedrive-config-diagnostic" role="alert">
                配置错误：<code>{auth.configuration.errorCode}</code>
              </p>
            ) : null}

            {auth.configurationPlan ? (
              <div className="onedrive-config-impact" role="alert">
                <strong>{auth.configurationPlan.effectiveChanged ? "配置将发生变化" : "有效配置没有变化"}</strong>
                <span>
                  {auth.configurationPlan.effectiveChanged
                    ? `${auth.configurationPlan.affectedLibraries} 个资源库会被暂停并隔离旧账号会话。`
                    : "提交不会退出账号或改变资源库状态。"}
                </span>
                <div>
                  <LiquidGlassButton className="plugin-action" onClick={auth.clearConfigurationPlan} tone="control" type="button">
                    取消
                  </LiquidGlassButton>
                  <LiquidGlassButton
                    className="plugin-action"
                    disabled={auth.busy}
                    onClick={() => void auth.commitConfiguration(Boolean(auth.configurationPlan?.effectiveChanged && auth.configurationPlan.requiresConfirmation)).catch(() => undefined)}
                    tone="primary"
                    type="button"
                  >
                    确认应用
                  </LiquidGlassButton>
                </div>
              </div>
            ) : null}
          </div>
        </details>
      ) : null}
    </section>
  );
}

function primaryStateTitle(action: PluginAuthController["primaryAction"]): string {
  switch (action) {
    case "enable":
      return "OneDrive 已停用";
    case "approve":
      return "需要批准权限";
    case "configure":
      return "需要配置 OneDrive";
    case "connect":
      return "尚未连接 OneDrive";
    case "wait":
      return "正在连接 OneDrive";
    case "retry":
      return "需要重新连接";
    case "manage":
      return "OneDrive 已连接";
    default:
      return "OneDrive Provider 不可用";
  }
}

function primaryStateDescription(auth: PluginAuthController): string {
  switch (auth.primaryAction) {
    case "enable":
      return "启用插件后即可连接账号。";
    case "approve":
      return "先批准 Provider 请求的权限。";
    case "configure":
      return auth.configuration?.readOnly
        ? "此部署需要通过环境变量提供 Client ID。"
        : "配置 Microsoft Entra Client ID 后即可连接。";
    case "connect":
      return "支持 OneDrive Personal 与 Business。";
    case "wait":
      return "请在 Microsoft 页面完成授权。";
    case "retry":
      return "当前会话需要重试或重新授权。";
    case "manage":
      return accountKindLabel(auth.plugin?.accountKind ?? null);
    default:
      return "正在自动检测已安装的插件。";
  }
}
