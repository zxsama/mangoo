import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { HostPluginProjection, OneDriveConfiguration } from "@megle/core-client";
import { OneDriveAccountSetup, OneDriveAccountSetupContent } from "../../src/features/plugins/OneDriveAccountSetup";
import type { PluginAuthController } from "../../src/features/plugins/usePluginAuth";
import { ManagedLibraryWizard } from "../../src/features/library/ManagedLibraryWizard";
import { OverlayProvider } from "../../src/design/overlays";

const pluginBase: HostPluginProjection = {
  pluginId: "com.megle.onedrive", selectedVersion: "2.0.0", knownGoodVersion: "2.0.0",
  enabledIntent: true, runtimeState: "ready", authState: "authenticated",
  capabilities: ["sync-provider.v1"], requestedPermissions: [], grantedPermissions: [], permissionRequired: false,
  accountUuid: "account-old", accountDisplayName: "Old account", accountKind: "personal",
  compatibilityReason: null, backoffUntil: null, lastHealthyAt: null, lastErrorCode: null, lastErrorDetail: null,
};
const configBase: OneDriveConfiguration = { source: "bundled", configured: true, readOnly: false, revision: 1, errorCode: null };
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
  if (path.endsWith("/configuration") && init?.method === "PUT") { commitBody = JSON.parse(String(init.body)); configuration = { ...configuration, revision: 2, source: "host-override" }; return json(configuration); }
  if (path.endsWith("/libraries/active")) return json({ contextRevision: activeContextRevision, library: null });
  if (path.endsWith("/sync/remote-libraries")) { remoteLibraryCalls += 1; return json({ items: [], nextCursor: null }); }
  return json({ accepted: true });
}) as typeof fetch;

window.open = () => ({ focus() {} }) as Window;

const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 0 }, mutations: { retry: false } } });
function Fixture() {
  const [mounted, setMounted] = useState(true);
  return <QueryClientProvider client={queryClient}><button id="toggle" onClick={() => setMounted((value) => !value)}>toggle</button>{mounted ? <OneDriveAccountSetup /> : null}</QueryClientProvider>;
}

function TransientInputFixture() {
  const [responseUri, setResponseUri] = useState("");
  const controller = {
    plugin: { ...pluginBase, authState: "authorizing" }, configuration: configBase, configurationPlan: null,
    primaryAction: "wait", flow: { authorizationUrl: "https://example.invalid", flowId: "transient", expiresAt: Date.now() + 60_000, completionMode: "manual-response-uri", redirectUri: null },
    responseUri, setResponseUri, busy: false, loading: false, error: null,
    refresh: async () => {}, enable: async () => {}, approve: async () => {}, retry: async () => {}, begin: async () => {}, openAuthorizationUrl: async () => {}, complete: async () => {}, cancel: async () => {}, signOut: async () => {}, copyAuthorizationUrl: async () => true, planConfiguration: async () => null, commitConfiguration: async () => {}, clearConfigurationPlan: () => {},
  } as PluginAuthController;
  return <OneDriveAccountSetupContent controller={controller} />;
}

function WizardFixture() {
  return <QueryClientProvider client={queryClient}><OverlayProvider><ManagedLibraryWizard initialMode="restore" onClose={() => {}} onCreated={() => {}} onFailure={() => {}} open /></OverlayProvider></QueryClientProvider>;
}

const root = createRoot(document.getElementById("root")!);
await act(async () => root.render(<Fixture />));

let stage = "initial render";
const waitFor = async (predicate: () => boolean) => {
  const deadline = Date.now() + 4_000;
  while (!predicate()) { if (Date.now() > deadline) throw new Error(`fixture wait timed out: ${stage}; ${document.body.textContent}`); await new Promise((resolve) => setTimeout(resolve, 20)); }
};
const button = (text: string) => [...document.querySelectorAll("button")].find((item) => item.textContent?.includes(text)) as HTMLButtonElement | undefined;
const click = async (text: string) => { const target = button(text); if (!target) throw new Error(`button not found: ${text}`); await act(async () => target.click()); };
const input = async (selector: string, value: string) => { const target = document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement; const setter = Object.getOwnPropertyDescriptor(target instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, "value")!.set!; await act(async () => { setter.call(target, value); target.dispatchEvent(new Event("input", { bubbles: true })); }); };

try {
  stage = "initial authenticated";
  await waitFor(() => Boolean(button("切换账号")));
  beginFails = true;
  stage = "switch failure";
  await click("切换账号"); await click("暂停并切换");
  await waitFor(() => document.body.textContent?.includes("已退出") === true);
  const switchFailure = { staleAccountHidden: !document.body.textContent?.includes("Old account"), signedOutVisible: document.body.textContent?.includes("已退出") === true };

  beginFails = false; cancelFails = true;
  stage = "begin manual";
  await click("连接 OneDrive"); await waitFor(() => Boolean(document.querySelector("textarea")));
  await click("取消授权"); await waitFor(() => document.body.textContent?.includes("cancel failed") === true);
  const cancelFailure = { flowRetained: Boolean(document.querySelector("textarea")), retryAvailable: Boolean(button("取消授权")) };

  cancelFails = false;
  stage = "complete manual";
  await input("textarea", "https://login.microsoftonline.com/common/oauth2/nativeclient?code=secret&state=s");
  await click("完成连接"); await waitFor(() => document.body.textContent?.includes("New account") === true);
  const manualCompletion = { dispatched: (completeBody as { flowId?: string })?.flowId === "flow-1", terminalCleared: !document.querySelector("textarea") };

  plugin = { ...plugin, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null };
  stage = "configuration editor";
  await queryClient.invalidateQueries({ queryKey: ["plugins"] });
  await waitFor(() => Boolean(button("高级配置")));
  await click("高级配置"); await input("input[placeholder]", "11111111-1111-1111-1111-111111111111");
  await click("预览更改"); await waitFor(() => document.body.textContent?.includes("配置将发生变化") === true);
  stage = "configuration commit";
  await click("确认应用"); await waitFor(() => configuration.revision === 2);
  await waitFor(() => !document.body.textContent?.includes("配置将发生变化"));
  const configConfirm = { confirmed: (commitBody as { confirmed?: boolean })?.confirmed === true };

  planChanged = false; commitBody = null; const priorPlanRequests = planRequestCount;
  stage = "configuration noop";
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
    button("预览更改")!.click();
  });
  await waitFor(() => planRequestCount > priorPlanRequests && document.body.textContent?.includes("有效配置没有变化") === true);
  await act(async () => { button("确认应用")!.click(); }); await waitFor(() => commitBody !== null);
  const configNoop = { committed: commitBody !== null };

  configuration = { ...configuration, readOnly: true, source: "environment" };
  stage = "configuration readonly";
  await queryClient.invalidateQueries({ queryKey: ["plugins", "com.megle.onedrive", "configuration"] });
  await waitFor(() => document.body.textContent?.includes("部署环境（只读）") === true);
  const readOnly = { editorHidden: !button("高级配置") };

  stage = "unmount setup";
  const transientHost = document.createElement("div"); document.body.append(transientHost);
  const transientRoot = createRoot(transientHost);
  await act(async () => transientRoot.render(<TransientInputFixture />));
  await input("textarea", "sensitive-response-uri");
  await act(async () => transientRoot.unmount());
  const remountedRoot = createRoot(transientHost);
  await act(async () => remountedRoot.render(<TransientInputFixture />));
  const unmountCleanup = { responseUriGone: !transientHost.textContent?.includes("sensitive-response-uri"), flowGone: (transientHost.querySelector("textarea") as HTMLTextAreaElement).value === "" };
  await act(async () => remountedRoot.unmount()); transientHost.remove();

  await act(async () => root.unmount()); queryClient.clear();
  plugin = { ...pluginBase, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null };
  configuration = { ...configBase }; activeContextRevision = 7; remoteLibraryCalls = 0;
  const wizardHost = document.createElement("div"); document.body.append(wizardHost); const wizardRoot = createRoot(wizardHost);
  await act(async () => wizardRoot.render(<WizardFixture />));
  stage = "wizard stable resume"; await waitFor(() => Boolean(button("连接 OneDrive")));
  await click("连接 OneDrive"); await waitFor(() => Boolean(document.querySelector("textarea")));
  await input("textarea", "https://login.microsoftonline.com/common/oauth2/nativeclient?code=wizard&state=s"); await click("完成连接");
  await waitFor(() => remoteLibraryCalls > 0 && document.activeElement?.textContent?.includes("从 OneDrive 恢复") === true);
  const wizardResume = { focusRestored: document.activeElement?.textContent?.includes("从 OneDrive 恢复") === true, remoteReloaded: remoteLibraryCalls > 0 };

  await act(async () => wizardRoot.unmount()); queryClient.clear();
  plugin = { ...pluginBase, authState: "authorization-required", accountUuid: null, accountDisplayName: null, accountKind: null }; activeContextRevision = 9;
  const staleWizardRoot = createRoot(wizardHost); await act(async () => staleWizardRoot.render(<WizardFixture />));
  stage = "wizard stale revision"; await waitFor(() => Boolean(button("连接 OneDrive"))); await click("连接 OneDrive"); await waitFor(() => Boolean(document.querySelector("textarea")));
  activeContextRevision = 10;
  await waitFor(() => document.body.textContent?.includes("资源库上下文已变化") === true);
  const wizardRevision = { staleIntentCleared: document.body.textContent?.includes("资源库上下文已变化") === true };
  await act(async () => staleWizardRoot.unmount()); wizardHost.remove();

  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: true, switchFailure, cancelFailure, manualCompletion, configConfirm, configNoop, readOnly, unmountCleanup, wizardResume, wizardRevision };
} catch (error) {
  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: false, error: error instanceof Error ? error.stack ?? error.message : String(error) };
}
