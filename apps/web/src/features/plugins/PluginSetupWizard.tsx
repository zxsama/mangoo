import { useEffect, useRef, useState, type RefObject } from "react";
import { Check, Clipboard, ExternalLink, Loader2 } from "../../design/icons";
import { copyText, getDesktopBridge } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayField, OverlayInlineStatus, OverlaySelect } from "../../design/overlays";
import type { OverlayRootId } from "../../design/overlays";
import { configurationSourceLabel, formatAuthRemaining } from "./pluginAuthModel";
import type { PluginAuthController } from "./usePluginAuth";
import { configurationFieldError, derivePluginSetupStep, visibleConfigurationFields, type PluginSetupStep } from "./pluginSetupModel";

const STEP_LABELS = ["配置说明", "保存配置", "登录验证", "完成"] as const;
interface Props {
  auth: PluginAuthController;
  initialStep?: PluginSetupStep;
  onBeforeBegin?: () => Promise<void> | void;
  onCancelled?: () => void;
  onFinish?: () => void;
  onOpenChange(open: boolean): void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
  rootId?: OverlayRootId;
}

export function PluginSetupWizard({ auth, initialStep, onBeforeBegin, onCancelled, onFinish, onOpenChange, open, restoreFocusRef, rootId = "library" }: Props) {
  const [step, setStep] = useState<PluginSetupStep>(1);
  const [checks, setChecks] = useState<boolean[]>([]);
  const [values, setValues] = useState<Record<string, string>>({});
  const [fieldErrors, setFieldErrors] = useState<Record<string, string | null>>({});
  const [configurationError, setConfigurationError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const firstActionRef = useRef<HTMLButtonElement>(null);
  const previousAuthenticatedRef = useRef(false);
  const authenticated = auth.plugin?.authState === "authenticated";
  const presentation = auth.plugin?.contributions?.presentation;
  const authentication = auth.plugin?.contributions?.authentication;
  const instructions = presentation?.setupSteps ?? [];
  const fields = visibleConfigurationFields(auth.configuration);
  const setupLabel = presentation?.setupLabel ?? `配置 ${auth.plugin?.name ?? "插件"}`;
  const accountLabel = presentation?.accountLabel ?? auth.plugin?.name ?? "账户";
  const allConfigurationChecksComplete = checks.every(Boolean);
  const manual = auth.flow?.completionMode === "manual-response-uri" || !getDesktopBridge();
  const redirect = manual ? authentication?.manualRedirectUri
    : authentication?.loopbackRedirectPath ? `http://localhost${authentication.loopbackRedirectPath}` : null;

  useEffect(() => {
    if (!open) return;
    const resumeStep = initialStep ?? derivePluginSetupStep(auth.configuration, authenticated);
    setStep(!authentication && resumeStep === 3 ? 2 : resumeStep);
    setChecks(instructions.map(() => false));
    setValues(auth.configuration?.values ?? {});
    setFieldErrors({}); setConfigurationError(null);
    auth.discardConfigurationPlan();
    previousAuthenticatedRef.current = authenticated;
  }, [open, auth.plugin?.pluginId]);
  useEffect(() => {
    const previous = previousAuthenticatedRef.current;
    previousAuthenticatedRef.current = authenticated;
    if (open && step === 3 && !previous && authenticated && auth.remoteAvailable) setStep(4);
  }, [authenticated, auth.remoteAvailable, open, step]);
  useEffect(() => {
    if (!auth.flow && !auth.configurationPlan) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [auth.flow, auth.configurationPlan]);

  const dismiss = () => { if (!auth.busy) { auth.discardConfigurationPlan(); onOpenChange(false); } };
  const advanceAfterConfiguration = () => setStep(authentication ? 3 : 4);
  const forwardLabel = step === 3 ? auth.flow?.completionMode === "manual-response-uri" ? "验证并连接" : auth.flow ? "等待授权" : `登录 ${accountLabel}` : step === 4 ? "完成" : "下一步";
  const forwardDisabled = auth.busy || (step === 1 && !allConfigurationChecksComplete)
    || (step === 2 && Boolean(auth.configurationPlan))
    || (step === 3 && (!auth.remoteAvailable || Boolean(auth.flow && (auth.flow.completionMode !== "manual-response-uri" || !auth.responseUri.trim()))));

  async function goForward() {
    if (step === 1) setStep(2);
    else if (step === 2) {
      if (auth.configuration?.readOnly) { if (auth.configuration.configured) advanceAfterConfiguration(); return; }
      const errors = Object.fromEntries(fields.map((field) => [field.id, configurationFieldError(field, auth.configuration?.lockedFields.includes(field.id) ? auth.configuration.values[field.id] ?? "" : values[field.id] ?? "")]));
      setFieldErrors(errors); setConfigurationError(null);
      if (Object.values(errors).some(Boolean)) return;
      try {
        const editableValues = Object.fromEntries(fields.filter((field) => !auth.configuration?.lockedFields.includes(field.id)).map((field) => [field.id, values[field.id]?.trim() || null]));
        const committed = await auth.saveConfiguration(editableValues);
        if (committed?.configured) advanceAfterConfiguration();
      } catch (cause) { setConfigurationError(cause instanceof Error ? cause.message : "无法保存配置，请重试。"); }
    } else if (step === 3) {
      if (!auth.flow) { await onBeforeBegin?.(); await auth.begin(); }
      else if (auth.flow.completionMode === "manual-response-uri") await auth.complete();
    } else { onOpenChange(false); onFinish?.(); }
  }

  return (
    <ModalSurface ariaDescribedBy="plugin-setup-description" ariaLabelledBy="plugin-setup-title" busy={auth.busy} className="plugin-setup-wizard" id="plugin-setup-wizard" initialFocusRef={firstActionRef} onDismiss={dismiss} open={open} restoreFocusRef={restoreFocusRef} rootId={rootId}>
      <OverlayDialogHeader description={["按插件提供的说明完成准备。", "配置仅保存在此设备。", "在浏览器中完成账户授权。", "配置已完成。"][step - 1]} descriptionId="plugin-setup-description" title={setupLabel} titleId="plugin-setup-title" />
      <ol aria-label="配置进度" className="plugin-setup-steps">
        {STEP_LABELS.map((label, index) => <li aria-current={index + 1 === step ? "step" : undefined} data-state={index + 1 < step ? "complete" : index + 1 === step ? "current" : "pending"} key={label}><span>{index + 1 < step ? <Check size={12} /> : index + 1}</span>{label}</li>)}
      </ol>
      <OverlayDialogBody className="plugin-setup-body" data-overlay-scroll-container="" data-step={step}>
        {step === 1 ? <section className="plugin-setup-section">
          <h3>配置说明</h3>
          <div className="plugin-auth-actions">{presentation?.helpLinks.map((link, index) => <LiquidGlassButton key={link.id} onClick={() => void auth.openHelp(link.id)} ref={index === 0 ? firstActionRef : undefined} type="button"><ExternalLink size={14} />{link.label}</LiquidGlassButton>)}</div>
          <div className="plugin-setup-confirmations">{instructions.map((instruction, index) => <label className="plugin-setup-confirm" key={index}>
            <input checked={checks[index] ?? false} onChange={(event) => setChecks((current) => current.map((checked, target) => target === index ? event.target.checked : checked))} type="checkbox" />
            <span><strong>{instruction.title}</strong><span>{instruction.body}</span></span>
          </label>)}</div>
          {instructions.length === 0 ? <p>此插件无需额外准备，可以继续配置。</p> : null}
          {redirect ? <p className="plugin-copy-value"><span>此设备的返回地址</span><code>{redirect}</code><LiquidGlassButton aria-label="复制返回地址" onClick={() => void copyText(redirect)} type="button"><Clipboard size={13} /></LiquidGlassButton></p> : null}
        </section> : null}
        {step === 2 ? <section className="plugin-setup-section plugin-setup-configuration">
          <h3>保存配置</h3>
          {auth.configuration ? <small>{configurationSourceLabel(auth.configuration.source)}</small> : null}
          {fields.map((field) => {
            const locked = auth.configuration?.lockedFields.includes(field.id) ?? false;
            const value = locked ? auth.configuration?.values[field.id] ?? "" : values[field.id] ?? "";
            const disabled = auth.busy || locked || Boolean(auth.configurationPlan);
            const update = (next: string) => { setValues((current) => ({ ...current, [field.id]: next })); setFieldErrors((current) => ({ ...current, [field.id]: null })); };
            return <div key={field.id}>
              {field.format === "choice" ? <><OverlaySelect aria-invalid={Boolean(fieldErrors[field.id])} disabled={disabled} label={field.label} onChange={(event) => update(event.target.value)} value={value}><option value="">请选择</option>{field.choices.map((choice) => <option key={choice} value={choice}>{choice}</option>)}</OverlaySelect>{fieldErrors[field.id] ? <OverlayInlineStatus tone="error">{fieldErrors[field.id]}</OverlayInlineStatus> : null}</>
                : <OverlayField autoComplete="off" className="plugin-configuration-input" disabled={disabled} error={fieldErrors[field.id]} label={field.label} onChange={(event) => update(event.target.value)} placeholder={field.description || field.label} spellCheck={false} value={value} />}
              {field.description ? <small>{field.description}</small> : null}
              {locked ? <OverlayInlineStatus tone="info">由部署环境管理（{field.environment}），请在部署设置中修改。</OverlayInlineStatus> : null}
            </div>;
          })}
          {fields.length === 0 ? <p>此插件无需用户配置。</p> : null}
          {configurationError ? <OverlayInlineStatus tone="error">{configurationError}</OverlayInlineStatus> : null}
          {auth.configurationPlan ? <div role="alert"><p>此修改将重置连接，影响 {auth.configurationPlan.affectedLibraries} 个资源库。确认后需要重新连接账户。</p><div className="plugin-auth-actions">
            <LiquidGlassButton disabled={auth.busy} onClick={() => auth.discardConfigurationPlan()} type="button">保留当前配置</LiquidGlassButton>
            <LiquidGlassButton disabled={auth.busy || auth.configurationPlan.expiresAt <= now} onClick={() => void auth.confirmConfiguration().then((committed) => { if (committed?.configured) advanceAfterConfiguration(); }).catch((cause) => setConfigurationError(cause instanceof Error ? cause.message : "无法保存配置。"))} tone="primary" type="button">确认修改</LiquidGlassButton>
          </div>{auth.configurationPlan.expiresAt <= now ? <p>确认已过期，请保留当前配置后重新保存。</p> : null}</div> : null}
        </section> : null}
        {step === 3 ? <section className="plugin-setup-section">
          <h3>登录并验证</h3>
          {!auth.remoteAvailable ? <OverlayInlineStatus tone="info">{auth.plugin?.runtimeState === "stopping" ? "插件正在停止，请稍后重试。" : "请在插件详情中启用插件后连接账户。"}</OverlayInlineStatus> : null}
          {!auth.flow ? <p>使用已有的 {accountLabel} 账户登录。</p> : <div className="plugin-auth-flow" aria-live="polite">
            <div><strong>在浏览器中完成授权</strong><span>剩余 {formatAuthRemaining(auth.flow.expiresAt, now)}</span></div>
            <div className="plugin-auth-actions"><LiquidGlassButton onClick={() => void auth.copyAuthorizationUrl()} type="button"><Clipboard size={14} />复制登录链接</LiquidGlassButton><LiquidGlassButton onClick={() => void auth.openAuthorizationUrl()} type="button"><ExternalLink size={14} />重新打开</LiquidGlassButton><LiquidGlassButton disabled={auth.busy} onClick={() => void auth.cancel().then(onCancelled).catch(() => undefined)} tone="danger" type="button">取消授权</LiquidGlassButton></div>
            {auth.flow.completionMode === "manual-response-uri" ? <div className="plugin-manual-completion"><label><span>登录返回地址</span><textarea autoComplete="off" onChange={(event) => auth.setResponseUri(event.target.value)} placeholder="粘贴浏览器地址栏中的完整返回地址" rows={3} spellCheck={false} value={auth.responseUri} /></label></div> : <small>授权完成后，Mangoo 会自动返回并更新状态。</small>}
          </div>}
        </section> : null}
        {step === 4 ? <section className="plugin-setup-section plugin-setup-centered plugin-setup-complete"><Check aria-hidden="true" size={24} /><h3>配置完成</h3><p><strong>{auth.plugin?.accountDisplayName ?? accountLabel}</strong></p></section> : null}
        {auth.error ? <OverlayInlineStatus tone="error">{auth.error}</OverlayInlineStatus> : null}
        {auth.busy ? <div className="plugin-setup-busy" role="status"><Loader2 className="spin" size={15} />正在处理…</div> : null}
      </OverlayDialogBody>
      <OverlayDialogActions className="plugin-setup-actions">
        {step < 4 ? <LiquidGlassButton disabled={auth.busy} onClick={dismiss} type="button">取消</LiquidGlassButton> : null}
        <LiquidGlassButton disabled={step === 1 || auth.busy} onClick={() => { auth.discardConfigurationPlan(); setStep((step - 1) as PluginSetupStep); }} type="button">上一步</LiquidGlassButton>
        <LiquidGlassButton disabled={forwardDisabled} onClick={() => void goForward().catch(() => undefined)} tone="primary" type="button">{step === 3 && !auth.flow ? <ExternalLink size={14} /> : null}{forwardLabel}</LiquidGlassButton>
      </OverlayDialogActions>
    </ModalSurface>
  );
}
