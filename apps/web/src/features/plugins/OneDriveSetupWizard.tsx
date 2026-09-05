import { useEffect, useMemo, useRef, useState, type Dispatch, type ReactNode, type RefObject, type SetStateAction } from "react";
import { Check, Clipboard, ExternalLink, Loader2 } from "lucide-react";
import { copyText, getDesktopBridge, openEntraAppRegistrationPage } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayField, OverlayInlineStatus } from "../../design/overlays";
import type { OverlayRootId } from "../../design/overlays";
import { accountKindLabel, configurationSourceLabel, formatAuthRemaining } from "./pluginAuthModel";
import type { PluginAuthController } from "./usePluginAuth";
import { deriveOneDriveSetupStep, isValidOneDriveClientId, oneDriveSetupRecoveryStep, type OneDriveSetupStep } from "./oneDriveSetupModel";

const ENTRA_REGISTRATION_URL = "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade";
const DESKTOP_REDIRECT = "http://localhost/plugin-auth/callback";
const DOCKER_REDIRECT = "https://login.microsoftonline.com/common/oauth2/nativeclient";
const STEP_LABELS = ["注册并配置应用", "保存 Client ID", "登录验证", "完成"] as const;

interface Props {
  auth: PluginAuthController;
  initialStep?: OneDriveSetupStep;
  onBeforeBegin?: () => Promise<void> | void;
  onCancelled?: () => void;
  onFinish?: () => void;
  onOpenChange(open: boolean): void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
  rootId?: OverlayRootId;
}

export function OneDriveSetupWizard({ auth, initialStep, onBeforeBegin, onCancelled, onFinish, onOpenChange, open, restoreFocusRef, rootId = "library" }: Props) {
  const [step, setStep] = useState<OneDriveSetupStep>(1);
  const [checks, setChecks] = useState([false, false, false, false]);
  const [clientId, setClientId] = useState("");
  const [clientIdError, setClientIdError] = useState<string | null>(null);
  const [registrationError, setRegistrationError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const firstActionRef = useRef<HTMLButtonElement>(null);
  const previousAuthenticatedRef = useRef(false);
  const authenticated = auth.plugin?.authState === "authenticated";
  const clientIdValid = isValidOneDriveClientId(clientId);
  const recoveryStep = oneDriveSetupRecoveryStep(auth.error);
  const allConfigurationChecksComplete = checks.every(Boolean);

  useEffect(() => {
    if (!open) return;
    setStep(initialStep ?? deriveOneDriveSetupStep(auth.configuration, authenticated));
    setChecks([false, false, false, false]);
    setClientId("");
    setClientIdError(null);
    setRegistrationError(null);
    previousAuthenticatedRef.current = authenticated;
  }, [open]);

  useEffect(() => {
    const previous = previousAuthenticatedRef.current;
    previousAuthenticatedRef.current = authenticated;
    if (open && step === 3 && !previous && authenticated) setStep(4);
  }, [authenticated, open, step]);

  useEffect(() => {
    if (!auth.flow) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [auth.flow]);

  const platform = auth.flow?.completionMode === "manual-response-uri" || !getDesktopBridge() ? "docker" : "desktop";
  const stepDescription = useMemo(() => [
    "按照当前 Microsoft Entra 页面完成应用注册和必要设置。",
    "从应用概述页复制客户端 ID 到 Megle。",
    "使用已有 Microsoft 账号完成授权。",
    "连接已就绪。",
  ][step - 1], [step]);

  const dismiss = () => {
    if (!auth.busy) {
      onOpenChange(false);
    }
  };

  async function openRegistrationPage() {
    setRegistrationError(null);
    if (getDesktopBridge()) {
      if (!await openEntraAppRegistrationPage()) setRegistrationError("无法打开 Microsoft Entra 应用注册页面，请检查系统浏览器后重试。");
      return;
    }
    if (!window.open(ENTRA_REGISTRATION_URL, "_blank", "noopener,noreferrer")) setRegistrationError("浏览器阻止了新标签页，请允许弹出窗口后重试。");
  }

  async function beginAuthentication() {
    await onBeforeBegin?.();
    await auth.begin();
  }

  function finish() {
    onOpenChange(false);
    onFinish?.();
  }

  const goBack = () => {
    if (step > 1 && !auth.busy) setStep((step - 1) as OneDriveSetupStep);
  };

  const forwardLabel = step === 3
      ? auth.flow?.completionMode === "manual-response-uri" ? "验证并连接" : auth.flow ? "等待授权" : "登录 Microsoft 账号"
      : step === 4 ? "完成" : "下一步";
  const forwardDisabled = auth.busy
    || (step === 1 && !allConfigurationChecksComplete)
    || (step === 3 && Boolean(auth.flow) && auth.flow?.completionMode !== "manual-response-uri")
    || (step === 3 && auth.flow?.completionMode === "manual-response-uri" && !auth.responseUri.trim());

  async function goForward() {
    if (step === 1) setStep(2);
    else if (step === 2) {
      if (auth.configuration?.readOnly) {
        setStep(3);
        return;
      }
      const normalizedClientId = clientId.trim();
      if (!normalizedClientId) {
        setClientIdError("请输入应用程序(客户端) ID。");
        return;
      }
      if (!clientIdValid) {
        setClientIdError("请输入 Microsoft Entra 生成的有效 UUID。");
        return;
      }
      setClientIdError(null);
      try {
        await auth.saveConfiguration(normalizedClientId);
        setStep(3);
      } catch (cause) {
        setClientIdError(cause instanceof Error ? cause.message : "无法保存 Client ID，请重试。");
      }
    } else if (step === 3) {
      if (!auth.flow) await beginAuthentication();
      else if (auth.flow.completionMode === "manual-response-uri") await auth.complete();
    } else finish();
  }

  return (
    <ModalSurface ariaDescribedBy="onedrive-setup-description" ariaLabelledBy="onedrive-setup-title" busy={auth.busy} className="onedrive-setup-wizard" id="onedrive-setup-wizard" initialFocusRef={firstActionRef} onDismiss={dismiss} open={open} restoreFocusRef={restoreFocusRef} rootId={rootId}>
      <OverlayDialogHeader description={stepDescription} descriptionId="onedrive-setup-description" title="配置 OneDrive 应用" titleId="onedrive-setup-title" />
      <ol aria-label="配置进度" className="onedrive-setup-steps">
        {STEP_LABELS.map((label, index) => {
          const number = (index + 1) as OneDriveSetupStep;
          const state = number < step ? "complete" : number === step ? "current" : "pending";
          return <li aria-current={number === step ? "step" : undefined} data-state={state} key={label}><span>{number < step ? <Check size={12} /> : number}</span>{label}</li>;
        })}
      </ol>
      <OverlayDialogBody
        className="onedrive-setup-body"
        data-overlay-scroll-container={step === 1 ? "" : undefined}
        data-step={step}
      >
        {step === 1 ? <section className="onedrive-setup-section">
          <h3>注册并配置 Microsoft Entra 应用</h3>
          <p>打开“应用注册”并选择“新注册”，然后按页面顺序完成以下设置。</p>
          <LiquidGlassButton onClick={() => void openRegistrationPage()} ref={firstActionRef} tone="primary" type="button"><ExternalLink size={14} />打开 Microsoft Entra</LiquidGlassButton>
          {registrationError ? <OverlayInlineStatus tone="error">{registrationError}</OverlayInlineStatus> : null}
          <div className="onedrive-setup-confirmations">
            <SetupConfirmation checked={checks[0]} label="新注册应用程序" onChange={(value) => updateCheck(setChecks, 0, value)} path="应用注册 → 新注册 → 新注册应用程序"><span><b>名称</b>：OneDrive Sync</span><span><b>受支持的帐户类型</b>：任何 Entra ID 租户 + 个人 Microsoft 帐户</span></SetupConfirmation>
            <SetupConfirmation checked={checks[1]} label="重定向 URI" onChange={(value) => updateCheck(setChecks, 1, value)} path="Authentication (Preview) → 重定向 URI 配置 → 添加重定向 URI → 移动和桌面应用程序 → 重定向 URI"><CopyableValue onCopy={async (value) => { await copyText(value); }} value={platform === "docker" ? DOCKER_REDIRECT : DESKTOP_REDIRECT} /></SetupConfirmation>
            <SetupConfirmation checked={checks[2]} label="允许公共客户端流" onChange={(value) => updateCheck(setChecks, 2, value)} path="Authentication (Preview) → 设置 → 允许公共客户端流 → 已启用" />
            <SetupConfirmation checked={checks[3]} label="API 权限" onChange={(value) => updateCheck(setChecks, 3, value)} path="API 权限 → 添加权限 → Microsoft Graph → 委托的权限 → Files.ReadWrite.AppFolder → 添加权限" />
          </div>
          {platform === "docker" ? <small>Docker 使用手动返回地址。</small> : null}
        </section> : null}

        {step === 2 ? <section className="onedrive-setup-section onedrive-setup-centered onedrive-setup-client-id">
          <h3>保存 Client ID</h3>
          {auth.configuration?.readOnly ? <OverlayInlineStatus tone="info">Client ID 由部署环境管理（{configurationSourceLabel(auth.configuration.source)}）。请设置有效的 MEGLE_ONEDRIVE_CLIENT_ID 并重启 Megle。</OverlayInlineStatus> : <>
            <p>在 Microsoft Entra 应用的“概述”页复制应用程序(客户端) ID。</p>
            <OverlayField
              autoComplete="off"
              className="onedrive-client-id-input"
              disabled={auth.busy}
              error={clientIdError}
              label="应用程序(客户端) ID"
              onChange={(event) => {
                setClientId(event.target.value);
                setClientIdError(null);
              }}
              placeholder="00000000-0000-4000-8000-000000000000"
              spellCheck={false}
              value={clientId}
            />
          </>}
        </section> : null}

        {step === 3 ? <section className="onedrive-setup-section">
          <h3>登录并验证</h3>
          {!auth.flow ? <p>将在系统浏览器中打开 Microsoft 登录页。使用已有的 Personal 或 Business 账号登录。</p> : <div className="onedrive-auth-flow" aria-live="polite">
            <div><strong>在 Microsoft 页面完成授权</strong><span>剩余 {formatAuthRemaining(auth.flow.expiresAt, now)}</span></div>
            <div className="onedrive-auth-actions"><LiquidGlassButton onClick={() => void auth.copyAuthorizationUrl()} type="button"><Clipboard size={14} />复制登录链接</LiquidGlassButton><LiquidGlassButton onClick={() => void auth.openAuthorizationUrl()} type="button"><ExternalLink size={14} />重新打开</LiquidGlassButton><LiquidGlassButton disabled={auth.busy} onClick={() => void auth.cancel().then(onCancelled).catch(() => undefined)} tone="danger" type="button">取消授权</LiquidGlassButton></div>
            {auth.flow.completionMode === "manual-response-uri" ? <div className="onedrive-manual-completion"><label><span>登录返回地址</span><textarea autoComplete="off" onChange={(event) => auth.setResponseUri(event.target.value)} placeholder="粘贴浏览器地址栏中的完整返回地址" rows={3} spellCheck={false} value={auth.responseUri} /></label></div> : <small>授权完成后，Megle 会自动返回并更新状态。</small>}
          </div>}
        </section> : null}

        {step === 4 ? <section className="onedrive-setup-section onedrive-setup-centered onedrive-setup-complete"><Check aria-hidden="true" size={24} /><h3>OneDrive 配置完成</h3><p><strong>{auth.plugin?.accountDisplayName ?? "Microsoft 账号"}</strong><span>{accountKindLabel(auth.plugin?.accountKind ?? null)}</span></p></section> : null}
        {auth.error && step !== 2 ? <OverlayInlineStatus tone="error"><span>{auth.error}</span>{recoveryStep && recoveryStep < step ? <LiquidGlassButton onClick={() => setStep(recoveryStep)} type="button">返回第 {recoveryStep} 步检查</LiquidGlassButton> : null}</OverlayInlineStatus> : null}
        {auth.busy ? <div className="onedrive-setup-busy" role="status"><Loader2 className="spin" size={15} />正在处理…</div> : null}
      </OverlayDialogBody>
      <OverlayDialogActions className="onedrive-setup-actions">
        {step < 4 ? <LiquidGlassButton disabled={auth.busy} onClick={dismiss} type="button">取消</LiquidGlassButton> : null}
        <LiquidGlassButton disabled={step === 1 || auth.busy} onClick={goBack} type="button">上一步</LiquidGlassButton>
        <LiquidGlassButton disabled={forwardDisabled} onClick={() => void goForward().catch(() => undefined)} tone="primary" type="button">{step === 3 && !auth.flow ? <ExternalLink size={14} /> : null}{forwardLabel}</LiquidGlassButton>
      </OverlayDialogActions>
    </ModalSurface>
  );
}

function updateCheck(setChecks: Dispatch<SetStateAction<boolean[]>>, target: number, value: boolean) {
  setChecks((current) => current.map((item, index) => index === target ? value : item));
}

function SetupConfirmation({ checked, children, label, onChange, path }: { checked: boolean; children?: ReactNode; label: string; onChange(value: boolean): void; path: string }) {
  return <label className="onedrive-setup-confirm"><input checked={checked} onChange={(event) => onChange(event.target.checked)} type="checkbox" /><span><strong>{label}</strong><span className="onedrive-setup-path">{path}</span>{children}</span></label>;
}

function CopyableValue({ onCopy, value }: { onCopy(value: string): Promise<void>; value: string }) {
  return <span className="onedrive-copy-value"><code>{value}</code><LiquidGlassButton aria-label={`复制 ${value}`} className="onedrive-copy-button" onClick={(event) => { event.preventDefault(); void onCopy(value); }} type="button"><Clipboard size={13} /></LiquidGlassButton></span>;
}
