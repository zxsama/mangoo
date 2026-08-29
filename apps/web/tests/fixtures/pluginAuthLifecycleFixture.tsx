import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { HostPluginProjection, OneDriveConfiguration } from "@megle/core-client";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { OneDriveAccountSetup, OneDriveAccountSetupContent } from "../../src/features/plugins/OneDriveAccountSetup";
import { PluginCard } from "../../src/features/plugins/PluginCard";
import { PluginDetail } from "../../src/features/plugins/PluginDetail";
import type { PluginAuthController } from "../../src/features/plugins/usePluginAuth";
import { ManagedLibraryWizard } from "../../src/features/library/ManagedLibraryWizard";
import { OverlayProvider } from "../../src/design/overlays";
import "../../src/styles.css";

type NativeInputPayload =
  | { type: "key"; keyCode: string; modifiers?: string[] }
  | { type: "mouse-click" | "mouse-move"; x: number; y: number };
type NativeInputRequest = NativeInputPayload & { id: number };

declare global {
  interface Window {
    __overlayLifecycleInputRequest?: NativeInputRequest;
    __overlayLifecycleInputResponseId?: number;
  }
}

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const pluginBase: HostPluginProjection = {
  pluginId: "com.megle.onedrive", name: "Megle OneDrive Sync Provider", selectedVersion: "2.0.0", knownGoodVersion: "2.0.0",
  enabledIntent: true, runtimeState: "ready", authState: "authenticated",
  capabilities: ["sync-provider.v1"], requestedPermissions: [], grantedPermissions: [], permissionRequired: false,
  accountUuid: "account-old", accountDisplayName: "Old account", accountKind: "personal",
  compatibilityReason: null, backoffUntil: null, lastHealthyAt: null, lastErrorCode: null, lastErrorDetail: null,
};
const configBase: OneDriveConfiguration = { source: "host-override", configured: true, readOnly: false, revision: 1, errorCode: null };
let plugin = { ...pluginBase };
let configuration = { ...configBase };
let beginFails = false;
let cancelFails = false;
let completeBody: unknown = null;
let commitBody: unknown = null;
let planChanged = true;
let planRequestCount = 0;
let activeContextRevision = 1;
let remoteLibraryCalls = 0;

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const url = new URL(String(input));
  const path = url.pathname;
  if (path.endsWith("/plugins") && (!init?.method || init.method === "GET")) return json({ items: [plugin] });
  if (path.endsWith("/configuration") && (!init?.method || init.method === "GET")) return json(configuration);
  if (path.endsWith("/auth/sign-out")) { plugin = { ...plugin, authState: "signed-out", accountUuid: null, accountDisplayName: null, accountKind: null }; return json({ accepted: true }); }
  if (path.endsWith("/auth/begin")) {
    if (beginFails) return json({ code: "begin_failed", message: "begin failed" }, 500);
    plugin = { ...plugin, authState: "authorizing" };
    return json({ authorizationUrl: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=test&response_type=code&code_challenge=x&code_challenge_method=S256&redirect_uri=https%3A%2F%2Flogin.microsoftonline.com%2Fcommon%2Foauth2%2Fnativeclient&scope=Files.ReadWrite.AppFolder&state=s", flowId: "flow-1", expiresAt: Date.now() + 60_000, completionMode: "manual-response-uri", redirectUri: "https://login.microsoftonline.com/common/oauth2/nativeclient" });
  }
  if (path.endsWith("/auth/cancel")) {
    if (cancelFails) return json({ code: "cancel_failed", message: "cancel failed" }, 500);
    plugin = { ...plugin, authState: "authorization-required" }; return json({ accepted: true });
  }
  if (path.endsWith("/auth/complete")) {
    completeBody = JSON.parse(String(init?.body));
    plugin = { ...plugin, authState: "authenticated", accountUuid: "new", accountDisplayName: "New account", accountKind: "business" };
    return json({ accepted: true });
  }
  if (path.endsWith("/configuration/plan")) { planRequestCount += 1; const changed = planChanged; return json({ planUuid: "plan-1", expectedRevision: 1, effectiveChanged: changed, affectedLibraries: changed ? 2 : 0, requiresConfirmation: changed, expiresAt: Date.now() + 60_000 }); }
  if (path.endsWith("/configuration") && init?.method === "PUT") { commitBody = JSON.parse(String(init.body)); configuration = { ...configuration, configured: true, errorCode: null, revision: 2, source: "host-override" }; return json(configuration); }
  if (path.endsWith("/libraries/active")) return json({ contextRevision: activeContextRevision, library: null });
  if (path.endsWith("/sync/remote-libraries")) { remoteLibraryCalls += 1; return json({ items: [], nextCursor: null }); }
  return json({ accepted: true });
}) as typeof fetch;

window.open = () => ({ focus() {} }) as Window;

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 0 }, mutations: { retry: false } } });
function Fixture() {
  const [mounted, setMounted] = useState(true);
  return <LiquidGlassLayer><QueryClientProvider client={queryClient}><OverlayProvider><button id="toggle" onClick={() => setMounted((value) => !value)}>toggle</button>{mounted ? <OneDriveAccountSetup /> : null}</OverlayProvider></QueryClientProvider></LiquidGlassLayer>;
}

function TransientInputFixture() {
  const [responseUri, setResponseUri] = useState("");
  const controller = {
    plugin: { ...pluginBase, authState: "authorizing" }, configuration: configBase, configurationPlan: null,
    primaryAction: "wait", flow: { authorizationUrl: "https://example.invalid", flowId: "transient", expiresAt: Date.now() + 60_000, completionMode: "manual-response-uri", redirectUri: null },
    responseUri, setResponseUri, busy: false, loading: false, error: null,
    refresh: async () => {}, enable: async () => {}, approve: async () => {}, retry: async () => {}, begin: async () => {}, openAuthorizationUrl: async () => {}, complete: async () => {}, cancel: async () => {}, signOut: async () => {}, copyAuthorizationUrl: async () => true, planConfiguration: async () => null, commitConfiguration: async () => {}, clearConfigurationPlan: () => {},
  } as PluginAuthController;
  return <OverlayProvider><OneDriveAccountSetupContent controller={controller} /></OverlayProvider>;
}

function WizardFixture() {
  return <QueryClientProvider client={queryClient}><OverlayProvider><ManagedLibraryWizard initialMode="restore" onClose={() => {}} onCreated={() => {}} onFailure={() => {}} open /></OverlayProvider></QueryClientProvider>;
}

function PluginPresentationFixture({ initialPlugin }: { initialPlugin: HostPluginProjection }) {
  const [selected, setSelected] = useState(false);
  const [enabled, setEnabled] = useState(initialPlugin.enabledIntent);
  const renderedPlugin = { ...initialPlugin, enabledIntent: enabled };
  return (
    <LiquidGlassLayer>
      <PluginCard busy={false} onSelect={() => setSelected(true)} onToggleEnabled={() => setEnabled((value) => !value)} plugin={renderedPlugin} selected={selected} />
      <PluginDetail busy={false} onApprovePermissions={() => {}} onRefresh={() => {}} onRemove={() => {}} onRetry={() => {}} plugin={renderedPlugin} />
    </LiquidGlassLayer>
  );
}

const root = createRoot(document.getElementById("root")!);
await act(async () => root.render(<Fixture />));

let stage = "initial render";
const waitFor = async (predicate: () => boolean) => {
  const deadline = Date.now() + 4_000;
  while (!predicate()) { if (Date.now() > deadline) throw new Error(`fixture wait timed out: ${stage}; active=${document.activeElement?.outerHTML}; ${document.body.textContent}`); await new Promise((resolve) => setTimeout(resolve, 20)); }
};
const button = (text: string) => [...document.querySelectorAll("button")].reverse().find((item) => item.textContent?.includes(text)) as HTMLButtonElement | undefined;
const wizardButton = (text: string) => [...document.querySelectorAll("#onedrive-setup-wizard button")].find((item) => item.textContent?.trim() === text) as HTMLButtonElement | undefined;
const control = (text: string) => [...document.querySelectorAll("button, summary")].reverse().find((item) => item.textContent?.includes(text)) as HTMLElement | undefined;
const click = async (text: string) => { const target = button(text); if (!target) throw new Error(`button not found: ${text}`); await act(async () => target.click()); };
const clickWizard = async (text: string) => { const target = wizardButton(text); if (!target) throw new Error(`wizard button not found: ${text}`); await act(async () => target.click()); };
const clickControl = async (text: string) => { const target = control(text); if (!target) throw new Error(`control not found: ${text}`); await act(async () => target.click()); };
const input = async (selector: string, value: string) => { const target = document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement; const setter = Object.getOwnPropertyDescriptor(target instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, "value")!.set!; await act(async () => { setter.call(target, value); target.dispatchEvent(new Event("input", { bubbles: true })); }); };
const delay = (milliseconds = 0) => new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));
let nativeInputRequestId = 0;
const requestNativeInput = async (request: NativeInputPayload) => {
  const id = ++nativeInputRequestId;
  window.__overlayLifecycleInputRequest = { ...request, id } as NativeInputRequest;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (window.__overlayLifecycleInputResponseId === id) return;
    await delay(10);
  }
  throw new Error(`Timed out waiting for native input response ${id}`);
};
const requestNativeKey = (keyCode: string) => requestNativeInput({ type: "key", keyCode });

try {
  stage = "initial authenticated";
  await waitFor(() => Boolean(button("切换账号")));
  stage = "switch opens wizard";
  await click("切换账号"); await click("退出并切换");
  await waitFor(() => document.body.textContent?.includes("登录并验证") === true);
  const switchFailure = { staleAccountHidden: !document.body.textContent?.includes("Old account"), signedOutVisible: document.body.textContent?.includes("登录并验证") === true };

  cancelFails = true;
  stage = "begin manual";
  await clickWizard("登录 Microsoft 账号");
  if (!document.querySelector("textarea")) await click("继续登录");
  await waitFor(() => Boolean(document.querySelector("textarea")));
  await clickWizard("取消授权"); await waitFor(() => document.body.textContent?.includes("cancel failed") === true);
  const cancelFailure = { flowRetained: Boolean(document.querySelector("textarea")), retryAvailable: Boolean(wizardButton("取消授权")) };

  cancelFails = false;
  stage = "complete manual";
  await input("textarea", "https://login.microsoftonline.com/common/oauth2/nativeclient?code=secret&state=s");
  await clickWizard("验证并连接"); await waitFor(() => document.body.textContent?.includes("OneDrive 配置完成") === true);
  const manualCompletion = { dispatched: (completeBody as { flowId?: string })?.flowId === "flow-1", terminalCleared: !document.querySelector("textarea") };
  await click("完成");

  plugin = { ...plugin, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null };
  configuration = { ...configuration, configured: false, source: "missing", errorCode: "onedrive-client-id-missing" };
  stage = "configuration editor";
  await queryClient.invalidateQueries({ queryKey: ["plugins"] });
  await queryClient.invalidateQueries({ queryKey: ["plugins", "com.megle.onedrive", "configuration"] });
  await waitFor(() => Boolean(button("配置 OneDrive")));
  await click("配置 OneDrive");
  await waitFor(() => document.body.textContent?.includes("创建 Microsoft Entra 应用") === true);
  const successfulWindowOpen = window.open;
  window.open = () => null;
  await click("打开 Microsoft Entra");
  await waitFor(() => document.body.textContent?.includes("浏览器阻止了新标签页") === true);
  const portalOpenFailureVisible = true;
  window.open = successfulWindowOpen;
  const nextDisabledBeforeConfirmation = button("下一步")?.disabled === true;
  const wizard = document.querySelector("#onedrive-setup-wizard") as HTMLElement;
  const firstStepHeight = wizard.getBoundingClientRect().height;
  const firstStepCheckbox = document.querySelector('.onedrive-setup-section input[type="checkbox"]') as HTMLInputElement;
  await act(async () => firstStepCheckbox.click());
  await click("下一步");
  const secondStepHeight = wizard.getBoundingClientRect().height;
  const setupCheckboxes = [...document.querySelectorAll('.onedrive-setup-section input[type="checkbox"]')] as HTMLInputElement[];
  const secondStepBlocked = button("下一步")?.disabled === true;
  for (const checkbox of setupCheckboxes) await act(async () => checkbox.click());
  await click("下一步");
  const thirdStepHeight = wizard.getBoundingClientRect().height;
  const configurationInput = document.querySelector('.onedrive-setup-section input[placeholder]') as HTMLInputElement;
  await input('.onedrive-setup-section input[placeholder]', "11111111-1111-4111-8111-111111111111");
  const wizardGates = { createBlocked: nextDisabledBeforeConfirmation, configurationBlocked: secondStepBlocked, clientIdAccepted: configurationInput.getAttribute("aria-invalid") !== "true" };
  const wizardLayout = {
    stableHeight: firstStepHeight === secondStepHeight && secondStepHeight === thirdStepHeight,
    bodyOwnsScroll: getComputedStyle(wizard.querySelector(".onedrive-setup-body") as HTMLElement).overflowY === "auto",
    footerVisible: (wizard.querySelector(".onedrive-setup-actions") as HTMLElement).getBoundingClientRect().bottom <= wizard.getBoundingClientRect().bottom,
    unifiedActions: !wizard.querySelector("button:not([data-liquid-glass='button'])")
  };
  await clickWizard("检查配置"); await waitFor(() => document.body.textContent?.includes("配置将发生变化") === true);
  await clickWizard("取消");
  await click("配置 OneDrive");
  const reopenedFirstStepCheckbox = document.querySelector('.onedrive-setup-section input[type="checkbox"]') as HTMLInputElement;
  await act(async () => reopenedFirstStepCheckbox.click());
  await click("下一步");
  const reopenedSetupCheckboxes = [...document.querySelectorAll('.onedrive-setup-section input[type="checkbox"]')] as HTMLInputElement[];
  for (const checkbox of reopenedSetupCheckboxes) await act(async () => checkbox.click());
  await click("下一步");
  const reopenState = { stalePlanGone: !document.body.textContent?.includes("配置将发生变化"), emptyClientIdBlocked: wizardButton("检查配置")?.disabled === true };
  await input('.onedrive-setup-section input[placeholder]', "11111111-1111-4111-8111-111111111111");
  await clickWizard("检查配置"); await waitFor(() => document.body.textContent?.includes("配置将发生变化") === true);
  stage = "configuration commit";
  await clickWizard("确认并继续"); await waitFor(() => configuration.revision === 2);
  await waitFor(() => document.body.textContent?.includes("登录并验证") === true);
  const configConfirm = { confirmed: (commitBody as { confirmed?: boolean })?.confirmed === true };

  stage = "configuration noop";
  planChanged = false;
  commitBody = null;
  await clickWizard("上一步");
  await input('.onedrive-setup-section input[placeholder]', "11111111-1111-4111-8111-111111111111");
  await clickWizard("检查配置");
  await waitFor(() => document.body.textContent?.includes("配置有效") === true);
  await clickWizard("确认并继续");
  await waitFor(() => document.body.textContent?.includes("登录并验证") === true);
  const configNoop = { committed: (commitBody as { confirmed?: boolean } | null)?.confirmed === false };
  planChanged = true;

  configuration = { ...configuration, readOnly: true, source: "environment" };
  stage = "configuration readonly";
  await queryClient.invalidateQueries({ queryKey: ["plugins", "com.megle.onedrive", "configuration"] });
  await waitFor(() => document.body.textContent?.includes("登录并验证") === true);
  await clickWizard("上一步");
  await waitFor(() => document.body.textContent?.includes("Client ID 由部署环境管理") === true);
  const readOnly = { editorHidden: !document.querySelector('.onedrive-setup-section input[placeholder]'), managedStatusVisible: true };
  await clickWizard("取消");

  stage = "unmount setup";
  const transientHost = document.createElement("div"); document.body.append(transientHost);
  const transientRoot = createRoot(transientHost);
  await act(async () => transientRoot.render(<TransientInputFixture />));
  await click("继续登录");
  await waitFor(() => Boolean(document.querySelector("textarea")));
  await input("textarea", "sensitive-response-uri");
  await act(async () => transientRoot.unmount());
  const remountedRoot = createRoot(transientHost);
  await act(async () => remountedRoot.render(<TransientInputFixture />));
  await click("继续登录");
  await waitFor(() => Boolean(document.querySelector("textarea")));
  const unmountCleanup = { responseUriGone: !document.body.textContent?.includes("sensitive-response-uri"), flowGone: (document.querySelector("textarea") as HTMLTextAreaElement).value === "" };
  await act(async () => remountedRoot.unmount()); transientHost.remove();

  await act(async () => root.unmount()); queryClient.clear();

  stage = "plugin presentation";
  const presentationPlugin: HostPluginProjection = {
    ...pluginBase,
    pluginId: "com.megle.presentation-check",
    name: "Presentation Check",
    runtimeState: "degraded",
    lastErrorCode: "provider-degraded",
    lastErrorDetail: "raw provider failure"
  };
  const presentationHost = document.createElement("div");
  presentationHost.style.cssText = "width:320px;margin:80px";
  document.body.append(presentationHost);
  const presentationRoot = createRoot(presentationHost);
  await act(async () => presentationRoot.render(<PluginPresentationFixture initialPlugin={presentationPlugin} />));
  await delay(40);
  const card = presentationHost.querySelector(".plugin-card") as HTMLElement;
  const cardBody = presentationHost.querySelector(".plugin-card-body") as HTMLElement;
  document.body.style.margin = "0";
  presentationHost.style.cssText = "width:320px;margin:80px";
  const cardBounds = card.getBoundingClientRect();
  document.documentElement.style.setProperty("--glass-pointer-sensing-distance", "80px");
  const proximityX = Math.round(cardBounds.right + 20);
  const proximityY = Math.round(cardBounds.top + cardBounds.height / 2);
  await act(async () => window.dispatchEvent(new MouseEvent("mousemove", { clientX: proximityX, clientY: proximityY })));
  await delay(30);
  const outsideInactive = card.getAttribute("data-interactive-pointer-target") !== "true" &&
    card.style.getPropertyValue("--interactive-pointer-opacity").trim() !== "1" &&
    cardBody.getAttribute("data-interactive-pointer-target") !== "true" &&
    cardBody.style.getPropertyValue("--interactive-pointer-opacity").trim() !== "1";
  await act(async () => window.dispatchEvent(new MouseEvent("mousemove", {
    clientX: Math.round(cardBounds.left + cardBounds.width / 2),
    clientY: Math.round(cardBounds.top + cardBounds.height / 2)
  })));
  await delay(30);
  const pointerOptOut = {
    declared: card.getAttribute("data-pointer-glow") === "off",
    outsideInactive,
    insideInactive: card.getAttribute("data-interactive-pointer-target") !== "true" &&
      card.style.getPropertyValue("--interactive-pointer-opacity").trim() !== "1" &&
      cardBody.getAttribute("data-interactive-pointer-target") !== "true" &&
      cardBody.style.getPropertyValue("--interactive-pointer-opacity").trim() !== "1"
  };
  await act(async () => cardBody.click());
  await waitFor(() => presentationHost.querySelector(".plugin-card-body")?.getAttribute("aria-selected") === "true");
  const toggleInput = presentationHost.querySelector('.plugin-toggle input[type="checkbox"]') as HTMLInputElement;
  await act(async () => toggleInput.click());
  await waitFor(() => !(presentationHost.querySelector('.plugin-toggle input[type="checkbox"]') as HTMLInputElement).checked);
  const currentCard = presentationHost.querySelector(".plugin-card") as HTMLElement;
  const currentCardBody = presentationHost.querySelector(".plugin-card-body") as HTMLElement;
  const currentToggleInput = presentationHost.querySelector('.plugin-toggle input[type="checkbox"]') as HTMLInputElement;
  const cardInteraction = { selected: currentCardBody.getAttribute("aria-selected") === "true", toggled: !currentToggleInput.checked };
  const cardText = currentCard.textContent ?? "";
  const cardParts = [...currentCardBody.children].map((child) => child.className);
  const technicalDetails = presentationHost.querySelector("details.plugin-technical-details") as HTMLDetailsElement;
  const technicalSummary = technicalDetails.querySelector("summary") as HTMLElement;
  const removeButton = [...technicalDetails.querySelectorAll("button")].find((item) => item.textContent?.includes("从注册表移除"));
  const conciseCard = {
    friendlyNameVisible: cardText.includes("Presentation Check"),
    rawIdHidden: !cardText.includes("com.megle.presentation-check"),
    oneLineOrder: cardParts.join("|") === "plugin-card-icon|plugin-card-name|plugin-card-version|plugin-status-pill plugin-status-pill-degraded",
    rawErrorHidden: !cardText.includes("raw provider failure"),
    toggleTextHidden: !cardText.includes("Disable"),
    toggleAccessible: currentToggleInput.getAttribute("aria-label") === "Enable"
  };
  const technicalDisclosure = {
    collapsedInitially: !technicalDetails.open,
    pointerGlowDisabled: technicalSummary.getAttribute("data-pointer-glow") === "off",
    chevronBeforeLabel: technicalSummary.firstElementChild?.classList.contains("plugin-disclosure-chevron") === true,
    diagnosticsNested: technicalDetails.textContent?.includes("raw provider failure") === true,
    removalNested: Boolean(removeButton)
  };
  const detailPanel = technicalDetails.closest(".plugin-detail");
  const tabRelationship = {
    controlsPanel: currentCardBody.getAttribute("aria-controls") === detailPanel?.id,
    panelLabelledByTab: detailPanel?.getAttribute("aria-labelledby") === currentCardBody.id,
    panelRole: detailPanel?.getAttribute("role") === "tabpanel"
  };
  await act(async () => technicalSummary.focus());
  await waitFor(() => document.activeElement === technicalSummary);
  await requestNativeKey("Space");
  await waitFor(() => technicalDetails.open);
  const technicalOpenedWithKeyboard = true;
  await act(async () => presentationRoot.unmount());
  presentationHost.remove();

  plugin = { ...pluginBase, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null };
  configuration = { ...configBase }; activeContextRevision = 7; remoteLibraryCalls = 0;
  const wizardHost = document.createElement("div"); document.body.append(wizardHost); const wizardRoot = createRoot(wizardHost);
  await act(async () => wizardRoot.render(<WizardFixture />));
  stage = "wizard stable resume"; await waitFor(() => Boolean(button("登录 Microsoft 账号")));
  await click("登录 Microsoft 账号"); await waitFor(() => Boolean(button("登录 Microsoft 账号")));
  await click("登录 Microsoft 账号"); await waitFor(() => Boolean(document.querySelector("textarea")));
  await input("textarea", "https://login.microsoftonline.com/common/oauth2/nativeclient?code=wizard&state=s"); await click("验证并连接");
  await waitFor(() => remoteLibraryCalls > 0 && document.activeElement?.textContent?.includes("从 OneDrive 恢复") === true);
  const wizardResume = { focusRestored: document.activeElement?.textContent?.includes("从 OneDrive 恢复") === true, remoteReloaded: remoteLibraryCalls > 0 };

  await act(async () => wizardRoot.unmount()); queryClient.clear();
  plugin = { ...pluginBase, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null }; activeContextRevision = 9;
  const staleWizardRoot = createRoot(wizardHost); await act(async () => staleWizardRoot.render(<WizardFixture />));
  stage = "wizard stale revision"; await waitFor(() => Boolean(button("登录 Microsoft 账号"))); await click("登录 Microsoft 账号"); await waitFor(() => Boolean(button("登录 Microsoft 账号"))); await click("登录 Microsoft 账号"); await waitFor(() => Boolean(document.querySelector("textarea")));
  activeContextRevision = 10;
  await waitFor(() => document.body.textContent?.includes("资源库上下文已变化") === true);
  const wizardRevision = { staleIntentCleared: document.body.textContent?.includes("资源库上下文已变化") === true };
  await act(async () => staleWizardRoot.unmount()); wizardHost.remove();

  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: true, switchFailure, cancelFailure, manualCompletion, configConfirm, configNoop, portalOpenFailureVisible, reopenState, wizardGates, wizardLayout, readOnly, unmountCleanup, pointerOptOut, cardInteraction, conciseCard, tabRelationship, technicalDisclosure, technicalOpenedWithKeyboard, wizardResume, wizardRevision };
} catch (error) {
  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: false, error: error instanceof Error ? error.stack ?? error.message : String(error) };
}
