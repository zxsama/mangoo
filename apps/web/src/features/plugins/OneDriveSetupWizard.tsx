import { useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Check, Clipboard, ExternalLink, Loader2 } from "lucide-react";
import { copyText, getDesktopBridge, openEntraAppRegistrationPage } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayInlineStatus,
} from "../../design/overlays";
import { accountKindLabel, configurationSourceLabel, formatAuthRemaining } from "./pluginAuthModel";
import type { PluginAuthController } from "./usePluginAuth";
import {
  deriveOneDriveSetupStep,
  isValidOneDriveClientId,
  oneDriveSetupRecoveryStep,
  type OneDriveSetupStep,
} from "./oneDriveSetupModel";

const ENTRA_REGISTRATION_URL = "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade";
const DESKTOP_REDIRECT = "http://127.0.0.1/plugin-auth/callback";
const DOCKER_REDIRECT = "https://login.microsoftonline.com/common/oauth2/nativeclient";

interface Props {
  affectedLibraries?: number;
  auth: PluginAuthController;
  initialStep?: OneDriveSetupStep;
  onBeforeBegin?: () => Promise<void> | void;
  onCancelled?: () => void;
  onFinish?: () => void;
  onOpenChange(open: boolean): void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

export function OneDriveSetupWizard({
  affectedLibraries = 0,
  auth,
  initialStep,
  onBeforeBegin,
  onCancelled,
  onFinish,
  onOpenChange,
  open,
  restoreFocusRef,
}: Props) {
  const [step, setStep] = useState<OneDriveSetupStep>(1);
  const [created, setCreated] = useState(false);
  const [checks, setChecks] = useState([false, false, false, false]);
  const [clientId, setClientId] = useState("");
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
    auth.clearConfigurationPlan();
    setStep(initialStep ?? deriveOneDriveSetupStep(auth.configuration, authenticated));
    setCreated(false);
    setChecks([false, false, false, false]);
    setClientId("");
    setRegistrationError(null);
    previousAuthenticatedRef.current = authenticated;
  }, [open]);

  useEffect(() => {
    const previous = previousAuthenticatedRef.current;
    previousAuthenticatedRef.current = authenticated;
    if (open && step === 4 && !previous && authenticated) setStep(5);
  }, [authenticated, open, step]);

  useEffect(() => {
    if (!auth.flow) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [auth.flow]);

  const platform = auth.flow?.completionMode === "manual-response-uri" || !getDesktopBridge() ? "docker" : "desktop";
  const stepDescription = useMemo(() => [
    "创建由你自己管理的 Microsoft Entra 公共客户端应用。",
    "按当前运行环境配置账户类型、公共客户端、回调地址和委托权限。",
    "只填写 Application (client) ID；不要填写目录 ID、对象 ID或客户端机密。",
    "在 Microsoft 官方页面登录已有账号并同意访问自己的 OneDrive 应用文件夹。",
    "OneDrive 已连接，可以返回之前的流程。",
  ][step - 1], [step]);

  const dismiss = () => {
    if (!auth.busy) {
      auth.clearConfigurationPlan();
      onOpenChange(false);
    }
  };

  async function openRegistrationPage() {
    setRegistrationError(null);
    if (getDesktopBridge()) {
      if (!await openEntraAppRegistrationPage()) {
        setRegistrationError("无法打开 Microsoft Entra 应用注册页面，请检查系统浏览器后重试。");
      }
      return;
    }
    if (!window.open(ENTRA_REGISTRATION_URL, "_blank", "noopener,noreferrer")) {
      setRegistrationError("浏览器阻止了新标签页，请允许弹出窗口后重试。");
    }
  }

  async function copyValue(value: string) {
    await copyText(value);
  }

  async function saveConfiguration() {
    await auth.planConfiguration(clientId.trim());
  }

  async function commitConfiguration() {
    const confirmed = Boolean(auth.configurationPlan?.effectiveChanged && auth.configurationPlan.requiresConfirmation);
    await auth.commitConfiguration(confirmed);
    setStep(4);
  }

  async function beginAuthentication() {
    await onBeforeBegin?.();
    await auth.begin();
  }

  function finish() {
    onOpenChange(false);
    onFinish?.();
  }

  return (
    <ModalSurface
      ariaDescribedBy="onedrive-setup-description"
      ariaLabelledBy="onedrive-setup-title"
      busy={auth.busy}
      className="onedrive-setup-wizard"
      id="onedrive-setup-wizard"
      initialFocusRef={firstActionRef}
      onDismiss={dismiss}
      open={open}
      restoreFocusRef={restoreFocusRef}
      rootId="library"
    >
      <OverlayDialogHeader
        description={stepDescription}
        descriptionId="onedrive-setup-description"
        title="配置 OneDrive 应用"
        titleId="onedrive-setup-title"
      />
      <OverlayDialogBody className="onedrive-setup-body">
        <ol aria-label="配置进度" className="onedrive-setup-steps">
          {["创建应用", "配置应用", "保存 Client ID", "登录验证", "完成"].map((label, index) => {
            const number = (index + 1) as OneDriveSetupStep;
            return <li aria-current={number === step ? "step" : undefined} data-state={number < step ? "complete" : number === step ? "current" : "pending"} key={label}><span>{number < step ? <Check size={12} /> : number}</span>{label}</li>;
          })}
        </ol>

        {step === 1 ? <section className="onedrive-setup-section">
          <h3>创建自己的 Entra 应用</h3>
          <p>该应用只标识你自己的 OAuth 配置。Microsoft 账号密码始终只输入在微软页面，Megle 不会收集密码、客户端机密或令牌。</p>
          <LiquidGlassButton onClick={() => void openRegistrationPage()} ref={firstActionRef} tone="primary" type="button"><ExternalLink size={14} />打开 Microsoft Entra 应用注册</LiquidGlassButton>
          {registrationError ? <OverlayInlineStatus tone="error">{registrationError}</OverlayInlineStatus> : null}
          <label className="onedrive-setup-confirm"><input checked={created} onChange={(event) => setCreated(event.target.checked)} type="checkbox" /><span>我已创建应用，并选择“任何组织目录中的账户和个人 Microsoft 账户”。</span></label>
        </section> : null}

        {step === 2 ? <section className="onedrive-setup-section">
          <h3>配置公共客户端</h3>
          <p>当前按 {platform === "docker" ? "Docker / Web 手动返回" : "Windows Desktop 自动回调"} 模式显示配置。</p>
          <SetupConfirmation checked={checks[0]} label="支持的账户类型" onChange={(value) => setChecks((current) => current.map((item, index) => index === 0 ? value : item))}>已选择“任何组织目录中的账户和个人 Microsoft 账户”。</SetupConfirmation>
          <SetupConfirmation checked={checks[1]} label="公共客户端" onChange={(value) => setChecks((current) => current.map((item, index) => index === 1 ? value : item))}>已在“身份验证”中启用公共客户端流；不创建 Client Secret。</SetupConfirmation>
          <SetupConfirmation checked={checks[2]} label="重定向 URI" onChange={(value) => setChecks((current) => current.map((item, index) => index === 2 ? value : item))}>
            已添加 <CopyableValue onCopy={copyValue} value={platform === "docker" ? DOCKER_REDIRECT : DESKTOP_REDIRECT} />{platform === "desktop" ? "；Megle 登录时会使用同一路径和临时本机端口。" : ""}
          </SetupConfirmation>
          <SetupConfirmation checked={checks[3]} label="委托权限" onChange={(value) => setChecks((current) => current.map((item, index) => index === 3 ? value : item))}>已添加 Microsoft Graph 委托权限 <code>Files.ReadWrite.AppFolder</code>。</SetupConfirmation>
          <small>组织策略可能要求管理员同意。Microsoft 当前将该委托权限标记为 Preview。</small>
        </section> : null}

        {step === 3 ? <section className="onedrive-setup-section">
          <h3>保存 Application (client) ID</h3>
          {auth.configuration?.readOnly ? <OverlayInlineStatus tone="info">Client ID 由部署环境管理（{configurationSourceLabel(auth.configuration.source)}），界面不能覆盖。请设置有效的 MEGLE_ONEDRIVE_CLIENT_ID 并重启 Megle。</OverlayInlineStatus> : <>
            <label className="overlay-field"><span>Application (client) ID</span><input aria-invalid={clientId.length > 0 && !clientIdValid} autoComplete="off" onChange={(event) => { setClientId(event.target.value); auth.clearConfigurationPlan(); }} placeholder="00000000-0000-4000-8000-000000000000" spellCheck={false} value={clientId} /></label>
            <small>{clientId.length === 0 || clientIdValid ? "这是公开的应用标识，不是 Directory ID、Object ID 或 Client Secret。" : "请输入 Microsoft Entra 生成的有效 UUID。"}</small>
            {!auth.configurationPlan ? <LiquidGlassButton disabled={!clientIdValid || auth.busy} onClick={() => void saveConfiguration().catch(() => undefined)} tone="primary" type="button">检查并保存</LiquidGlassButton> : null}
            {auth.configurationPlan ? <div className="onedrive-config-impact" role="status"><strong>{auth.configurationPlan.effectiveChanged ? "配置将发生变化" : "有效配置没有变化"}</strong><span>{auth.configurationPlan.effectiveChanged ? `${auth.configurationPlan.affectedLibraries || affectedLibraries} 个资源库可能暂停，旧账号凭据将安全退出。` : "不会退出账号或改变资源库状态。"}</span><div><button onClick={auth.clearConfigurationPlan} type="button">返回修改</button><LiquidGlassButton disabled={auth.busy} onClick={() => void commitConfiguration().catch(() => undefined)} tone="primary" type="button">确认应用</LiquidGlassButton></div></div> : null}
          </>}
        </section> : null}

        {step === 4 ? <section className="onedrive-setup-section">
          <h3>登录并验证</h3>
          {!auth.flow ? <><p>点击后将在系统浏览器打开 Microsoft 登录页面。请使用已有的 Personal 或 Business 账号登录。</p><LiquidGlassButton disabled={auth.busy} onClick={() => void beginAuthentication().catch(() => undefined)} tone="primary" type="button"><ExternalLink size={14} />登录 Microsoft 账号</LiquidGlassButton></> : <div className="onedrive-auth-flow" aria-live="polite">
            <div><strong>在 Microsoft 页面完成授权</strong><span>剩余 {formatAuthRemaining(auth.flow.expiresAt, now)}</span></div>
            <div className="onedrive-auth-actions"><button onClick={() => void auth.copyAuthorizationUrl()} type="button"><Clipboard size={14} />复制登录链接</button><button onClick={() => void auth.openAuthorizationUrl()} type="button"><ExternalLink size={14} />重新打开</button><button disabled={auth.busy} onClick={() => void auth.cancel().then(onCancelled).catch(() => undefined)} type="button">取消</button></div>
            {auth.flow.completionMode === "manual-response-uri" ? <form className="onedrive-manual-completion" onSubmit={(event) => { event.preventDefault(); void auth.complete().catch(() => undefined); }}><label><span>登录返回地址</span><textarea autoComplete="off" onChange={(event) => auth.setResponseUri(event.target.value)} placeholder="完成登录后，复制浏览器地址栏中的完整地址并粘贴到这里" rows={3} spellCheck={false} value={auth.responseUri} /></label><LiquidGlassButton disabled={auth.busy || !auth.responseUri.trim()} tone="primary" type="submit">验证并连接</LiquidGlassButton></form> : <small>授权完成后 Megle 会自动更新并尝试返回到此窗口。</small>}
          </div>}
        </section> : null}

        {step === 5 ? <section className="onedrive-setup-section onedrive-setup-complete"><Check aria-hidden="true" size={24} /><h3>OneDrive 配置完成</h3><p><strong>{auth.plugin?.accountDisplayName ?? "Microsoft 账号"}</strong><span>{accountKindLabel(auth.plugin?.accountKind ?? null)}</span></p></section> : null}

        {auth.error ? <OverlayInlineStatus tone="error"><span>{auth.error}</span>{recoveryStep && recoveryStep < step ? <button onClick={() => setStep(recoveryStep)} type="button">返回第 {recoveryStep} 步检查</button> : null}</OverlayInlineStatus> : null}
        {auth.busy ? <div className="onedrive-setup-busy" role="status"><Loader2 className="spin" size={15} />正在处理…</div> : null}
      </OverlayDialogBody>
      <OverlayDialogActions>
        {step < 5 ? <button disabled={auth.busy} onClick={dismiss} type="button">取消</button> : null}
        {step > 1 && step < 4 ? <button disabled={auth.busy} onClick={() => setStep((step - 1) as OneDriveSetupStep)} type="button">上一步</button> : null}
        {step === 1 ? <LiquidGlassButton disabled={!created} onClick={() => setStep(2)} type="button">下一步</LiquidGlassButton> : null}
        {step === 2 ? <LiquidGlassButton disabled={!allConfigurationChecksComplete} onClick={() => setStep(auth.configuration?.configured ? 4 : 3)} type="button">下一步</LiquidGlassButton> : null}
        {step === 3 && auth.configuration?.readOnly && auth.configuration.configured ? <LiquidGlassButton onClick={() => setStep(4)} type="button">下一步</LiquidGlassButton> : null}
        {step === 4 && !auth.flow ? <button disabled={auth.busy} onClick={() => setStep(3)} type="button">检查 Client ID</button> : null}
        {step === 5 ? <><button onClick={() => setStep(3)} type="button">更换 Client ID</button><LiquidGlassButton onClick={finish} type="button">完成</LiquidGlassButton></> : null}
      </OverlayDialogActions>
    </ModalSurface>
  );
}

function SetupConfirmation({ checked, children, label, onChange }: { checked: boolean; children: ReactNode; label: string; onChange(value: boolean): void }) {
  return <label className="onedrive-setup-confirm"><input checked={checked} onChange={(event) => onChange(event.target.checked)} type="checkbox" /><span><strong>{label}</strong>{children}</span></label>;
}

function CopyableValue({ onCopy, value }: { onCopy(value: string): Promise<void>; value: string }) {
  return <span className="onedrive-copy-value"><code>{value}</code><button aria-label={`复制 ${value}`} onClick={(event) => { event.preventDefault(); void onCopy(value); }} type="button"><Clipboard size={13} /></button></span>;
}
