import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { HostPluginProjection } from "@mangoo/core-client";
import type { DesktopNotificationCompactSnapshot, DesktopNotificationsBridge, NotificationPluginOpenRequest, PluginViewWindowIntent } from "@mangoo/notification-contract";
import { OverlayProvider } from "../../src/design/overlays";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import { PluginContributionsSlot } from "../../src/features/plugins/PluginContributionsSlot";
import { PluginDetail } from "../../src/features/plugins/PluginDetail";
import { PluginsWindow } from "../../src/features/plugins/PluginsWindow";
import { NotificationWindow } from "../../src/features/notifications/NotificationWindow";
import { NotificationPopover } from "../../src/features/notifications/NotificationPopover";
import { buildCompactNotificationModel } from "../../src/features/notifications/notificationModel";
import "../../src/styles.css";

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const plugin: HostPluginProjection = {
  pluginId: "org.example.extension", lifecycleEpoch: 1, name: "Example", selectedVersion: "1.0.0", knownGoodVersion: "1.0.0", enabledIntent: true, runtimeState: "ready", authState: "authenticated", accountUuid: "account-a", accountDisplayName: null, accountKind: null, capabilities: [], permissionRequired: false, requestedPermissions: [], grantedPermissions: [{ kind: "host-ui", operations: ["core-api", "clipboard-write"] }], compatibilityReason: null, backoffUntil: null, lastHealthyAt: null, lastErrorCode: null, lastErrorDetail: null,
  contributions: { configuration: [], authentication: null, presentation: null, primaryCredential: null, ui: { apiVersion: 1, entry: "ui.html", sha256: "a".repeat(64), views: [{ id: "main", title: "Example view", slot: "library-settings" }] } },
};
const documentSource = `<style>body{color:#f4f7f9;font:14px sans-serif;padding:20px}</style><h1>Example plugin</h1><button id="launch">Open inner dialog</button><dialog id="inner"><button id="dismiss">Close inner dialog</button></dialog><script>
let session;let sequence=0;const pending=new Map();
function call(method,args=[]){return new Promise((resolve,reject)=>{const requestId=String(++sequence);pending.set(requestId,{resolve,reject});parent.postMessage({type:'mangoo-plugin-ui:request',sessionId:session,requestId,method,args},'*')})}
document.getElementById('launch').onclick=()=>document.getElementById('inner').showModal();
document.getElementById('dismiss').onclick=()=>document.getElementById('inner').close();
document.addEventListener('focusin',()=>{if(session)void call('host.copyText',[JSON.stringify({focused:document.activeElement.id})])});
document.addEventListener('keydown',event=>{if(session)void call('host.copyText',[JSON.stringify({key:event.key,focused:document.activeElement.id})])});
window.addEventListener('message',async(event)=>{const m=event.data;if(m.type==='fixture-configure'){await call('core.commitPluginConfiguration',['org.example.extension',{planUuid:'plan',expectedRevision:1,confirmed:true}]);return}if(m.type==='mangoo-plugin-ui:response'){const p=pending.get(m.requestId);if(p){pending.delete(m.requestId);m.error?p.reject(m.error):p.resolve(m.result)}return}if(m.type!=='mangoo-plugin-ui:init'||session)return;session=m.sessionId;
let isolated=false;try{void parent.document.body}catch{isolated=true}let networkBlocked=false;try{await fetch('https://example.invalid/no-network')}catch{networkBlocked=true}
let denied=false;try{await call('core.fetch',['https://example.invalid'])}catch{denied=true}
const plan=await call('core.preflightSyncFolderEnrollment',['library-a',{mode:'enroll-existing',path:'C:/Picked',operationUuid:'operation-a',expectedOperationRevision:0}]);
await call('core.commitSyncFolderEnrollment',['library-a',1,{operationUuid:'operation-a',expectedOperationRevision:0,rootUuid:'root-a',rootFolderUuid:'folder-a',preflightToken:plan.preflightToken,path:plan.normalizedPath,requestedName:'Picked'}]);
await call('host.copyText',[JSON.stringify({isolated,networkBlocked,denied,noPreload:typeof window.mangooDesktop==='undefined',noToken:!JSON.stringify(m).includes('fixture-private-token'),theme:m.theme.colorScheme})]);document.body.dataset.complete='true';
});parent.postMessage({type:'mangoo-plugin-ui:ready'},'*');
</script>`;
let installed = false;
let mismatchDocument = false;
const notificationSnapshot: DesktopNotificationCompactSnapshot = {
  active: [], thumbnailSummaries: [], latestImportantResults: [], hasRetryableFailure: false,
  syncHealth: { state: 'degraded', libraryId: 'library-a', detail: 'Local notification store unavailable', updatedAt: 1 },
};
let notificationChanged: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
let notificationEvidence: Record<string, unknown> | null = null;
const notificationDocument = `<h1>Example plugin status</h1><p id="status" role="status">Loading</p><script>
let session;let sequence=0;const pending=new Map();
function call(method,args=[]){return new Promise((resolve,reject)=>{const requestId=String(++sequence);pending.set(requestId,{resolve,reject});parent.postMessage({type:'mangoo-plugin-ui:request',sessionId:session,requestId,method,args},'*')})}
window.addEventListener('message',async(event)=>{const m=event.data;if(m.type==='mangoo-plugin-ui:response'){const p=pending.get(m.requestId);if(p){pending.delete(m.requestId);m.error?p.reject(m.error):p.resolve(m.result)}return}if(m.type!=='mangoo-plugin-ui:init'||session)return;session=m.sessionId;
let isolated=false;try{void parent.document.body}catch{isolated=true}
const status=await call('core.getSyncStatus');document.getElementById('status').textContent='Plugin queue: '+status.schedulerState;
await call('host.copyText',[JSON.stringify({notificationStatus:document.getElementById('status').textContent,libraryUuid:m.context.libraryUuid,viewId:m.viewId,isolated})]);
});parent.postMessage({type:'mangoo-plugin-ui:ready'},'*');
</script>`;
const requests: string[] = [], flow: string[] = [];
let epochPinned = true;
let evidence: Record<string, unknown> | null = null;
let frameFocus: string | null = null;
const frameKeys: string[] = [];
window.mangooDesktop = { sessionToken: "fixture-private-token", shell: { revealPath: async () => false, openPath: async () => false, copyText: async (value) => { const next = JSON.parse(value); if (next.notificationStatus) notificationEvidence = next; else { if (next.key) frameKeys.push(next.key); if (typeof next.focused === "string") frameFocus = next.focused; else evidence = next; } return true; }, openPluginExternalLink: async () => ({ ok: true }) } };
const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });
window.fetch = async (input, init) => {
  const path = new URL(String(input)).pathname; requests.push(path);
  if (path.endsWith('/plugins')) return json({ items: installed ? [plugin] : [] });
  if (path.endsWith('/plugins/org.example.extension')) return json(plugin);
  if (path.endsWith('/views/main')) return json({ pluginId: plugin.pluginId, version: plugin.selectedVersion, lifecycleEpoch: plugin.lifecycleEpoch + (mismatchDocument ? 1 : 0), viewId: 'main', title: 'Example view', document: documentSource });
  if (path.endsWith('/views/notification-status')) return json({ pluginId: plugin.pluginId, version: plugin.selectedVersion, lifecycleEpoch: plugin.lifecycleEpoch, viewId: 'notification-status', title: 'Example status', document: notificationDocument });
  if (path.endsWith('/configuration') && init?.method === 'PUT') { plugin.lifecycleEpoch++; return json({ accepted: true }); }
  if (path.endsWith('/libraries/active')) return json({ library: { libraryUuid: 'library-a' }, contextRevision: 1, runtimeGeneration: 1 });
  if (path.endsWith('/sync/status')) return json({ revision: 1, generatedAt: 1, schedulerState: 'running', providers: [], libraries: [{ libraryUuid: 'library-a', providerId: plugin.pluginId, accountUuid: plugin.accountUuid }] });
  if (path.includes('/enrollment/')) { const headers = new Headers(init?.headers); epochPinned &&= headers.get('x-mangoo-plugin-ui-id') === plugin.pluginId && headers.get('x-mangoo-plugin-ui-epoch') === String(plugin.lifecycleEpoch); }
  if (path.endsWith('/enrollment/preflight')) { flow.push('preflight'); return json({ ok: true, preflightToken: 'verified-plan', normalizedPath: 'C:/Picked' }); }
  if (path.endsWith('/enrollment/commit')) { const body = JSON.parse(String(init?.body)); if (body.preflightToken !== 'verified-plan') throw new Error('Missing preflight'); if (new Headers(init?.headers).get('x-mangoo-context-revision') !== '1') throw new Error('Missing pinned revision'); flow.push('commit'); return json({ accepted: true }); }
  throw new Error('Unexpected request '+path);
};
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
const root = createRoot(document.getElementById('root')!);
const delay = () => new Promise((resolve) => setTimeout(resolve, 20));
async function waitFor(test: () => boolean) { const deadline = Date.now() + 5000; while (!test()) { if (Date.now() > deadline) throw new Error('Plugin fixture timeout; focus='+document.activeElement?.tagName+'; frameFocus='+frameFocus+'; keys='+frameKeys.join(',')+'; input='+inputId+' '+JSON.stringify(requests)); await delay(); } }
const inputWindow = window as unknown as { __overlayLifecycleInputRequest?: { id: number; type: "cdp-key"; keyCode: string }; __overlayLifecycleInputResponseId?: number };
let inputId = 0;
async function key(keyCode: string) {
  const id = ++inputId;
  inputWindow.__overlayLifecycleInputRequest = { id, type: "cdp-key", keyCode };
  await waitFor(() => inputWindow.__overlayLifecycleInputResponseId === id);
}
try {
  await act(async () => root.render(<QueryClientProvider client={queryClient}><LiquidGlassLayer><OverlayProvider><PluginContributionsSlot slot="library-settings" libraryUuid="library-a" /></OverlayProvider></LiquidGlassLayer></QueryClientProvider>));
  await waitFor(() => requests.length > 0); await delay();
  const absent = { noFrame: document.querySelector('iframe') === null, noActions: document.querySelector('button') === null, noSyncRequests: requests.every((p) => !p.includes('/sync')) };
  installed = true; await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => Boolean(document.querySelector('button')));
  await act(async () => document.querySelector<HTMLButtonElement>('button')!.click());
  await waitFor(() => evidence !== null);
  const frame = document.querySelector('iframe')!;
  const sandbox = frame.getAttribute('sandbox');
  const modal = document.querySelector('#plugin-view')!;
  const modalShared = modal.classList.contains('overlay-modal-surface');
  const close = modal.querySelector<HTMLButtonElement>('.overlay-dialog-actions button')!;
  close.focus();
  await waitFor(() => document.activeElement === close);
  await key('Tab');
  const enteredFrame = document.activeElement === frame;
  if (!enteredFrame) throw new Error('Host focus trap prevented keyboard entry into plugin iframe');
  if (frameFocus !== 'launch') await key('Tab');
  await waitFor(() => frameFocus === 'launch');
  await key('Enter');
  await waitFor(() => frameFocus === 'dismiss');
  await key('Escape');
  await waitFor(() => frameFocus === 'launch');
  const innerEscapeKeptHost = document.querySelector('#plugin-view') === modal;
  await key('Tab');
  await waitFor(() => document.activeElement === close);
  const keyboard = { enteredFrame, innerEscapeKeptHost, returnedToHost: document.activeElement === close };
  const initialFlow = [...flow];
  frame.contentWindow!.postMessage({ type: 'fixture-configure' }, '*');
  await waitFor(() => Boolean(document.querySelector('#plugin-view-updated')));
  const updatedNotice = document.querySelector('iframe') === null && document.body.textContent!.includes('插件状态已更新，请重新打开视图继续');
  await act(async () => document.querySelector<HTMLButtonElement>('#plugin-view-updated button')!.click());
  mismatchDocument = true;
  await act(async () => document.querySelector<HTMLButtonElement>('button')!.click());
  await waitFor(() => document.body.textContent!.includes('Plugin view changed while loading'));
  const changedDocumentRejected = document.querySelector('iframe') === null;
  await act(async () => document.querySelector<HTMLButtonElement>('#plugin-view .overlay-dialog-actions button')!.click());
  mismatchDocument = false;
  evidence = null;
  await act(async () => document.querySelector<HTMLButtonElement>('button')!.click());
  await waitFor(() => evidence !== null);
  plugin.enabledIntent = false;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => document.querySelector('iframe') === null);
  const configurationReads = requests.filter((path) => path.endsWith('/configuration')).length;
  const setupPlugin: HostPluginProjection = { ...plugin, permissionRequired: true, contributions: { ...plugin.contributions!, configuration: [{ id: 'app', label: 'Application', description: 'Example', format: 'text', required: true, environment: 'EXAMPLE_APP', defaultValue: null, choices: [], hidden: false, debugOnly: false }] } };
  await act(async () => root.render(<QueryClientProvider client={queryClient}><LiquidGlassLayer><OverlayProvider><PluginDetail plugin={setupPlugin} busy={false} onApprovePermissions={() => {}} onToggleEnabled={() => {}} onRetry={() => {}} onRemove={() => {}} onRefresh={() => {}} /></OverlayProvider></LiquidGlassLayer></QueryClientProvider>));
  await delay();
  const pluginOwnedSetup = { noConfigurationReads: requests.filter((path) => path.endsWith('/configuration')).length === configurationReads, explicitEnable: Boolean(document.querySelector('input[type="checkbox"]')), explicitGrants: document.body.textContent!.includes('Approve permissions'), noHostSetup: document.querySelector('.plugin-account-setup') === null };
  const unexpectedNotificationCall = () => { throw new Error('Unexpected notification action'); };
  const notificationBridge: DesktopNotificationsBridge = {
    getCompactSnapshot: async () => notificationSnapshot,
    onChanged: (callback) => { notificationChanged = callback; return () => { notificationChanged = null; }; },
    getHistoryFacets: unexpectedNotificationCall, listHistory: unexpectedNotificationCall,
    cancel: unexpectedNotificationCall, retry: unexpectedNotificationCall,
    dismissManagedRoot: unexpectedNotificationCall, reviewManagedRoot: unexpectedNotificationCall,
    reportError: unexpectedNotificationCall, publishThumbnailSummary: unexpectedNotificationCall,
    getLogs: unexpectedNotificationCall, exportLogs: unexpectedNotificationCall,
    snapshotAndSubscribeLogs: unexpectedNotificationCall, clearLogs: unexpectedNotificationCall,
    emitRendererLogs: unexpectedNotificationCall,
  };
  window.mangooDesktop.notifications = notificationBridge;
  installed = false;
  const notificationRequestsStart = requests.length;
  await act(async () => root.render(<QueryClientProvider client={queryClient}><OverlayProvider><NotificationPopover anchor={document.getElementById('root')!} model={buildCompactNotificationModel({ ...notificationSnapshot, syncHealth: { ...notificationSnapshot.syncHealth, detail: null } })} onClose={() => {}} onOpenCenter={() => {}} /><NotificationWindow /></OverlayProvider></QueryClientProvider>));
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => document.body.textContent!.includes('Local notification store unavailable'));
  const notificationAbsent = {
    noSection: document.querySelector('.notification-status-plugins') === null,
    noFrame: document.querySelector('iframe') === null,
    noSyncEntry: !document.querySelector('#notification-panel-status')!.textContent!.includes('同步'),
    localErrorVisible: document.querySelector('.notification-status-health')!.textContent!.includes('通知服务受限') && document.querySelector('.notification-status-health')!.textContent!.includes('Local notification store unavailable'),
    compactServiceLabel: document.querySelector('.notification-popover')!.textContent!.includes('通知服务受限') && document.querySelector('.notification-popover')!.textContent!.includes('等待通知服务恢复') && document.querySelector('.notification-popover')!.textContent!.includes('通知服务暂时不可用'),
    noMisleadingHealthLabel: !document.body.textContent!.includes('同步受限'),
  };
  plugin.enabledIntent = true;
  installed = true;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  const noMatchingSlot = document.querySelector('.notification-status-plugins') === null;
  plugin.contributions!.ui!.views.push({ id: 'notification-status', title: 'Example status', slot: 'notification-center' });
  plugin.permissionRequired = true;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  const missingGrantHidden = document.querySelector('.notification-status-plugins') === null;
  plugin.permissionRequired = false;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  const notificationButton = () => document.querySelector<HTMLButtonElement>('.notification-status-plugins button');
  await waitFor(() => Boolean(notificationButton()));
  const noHostSyncRequests = requests.slice(notificationRequestsStart).every((path) => !path.includes('/sync'));
  const noEagerDocument = requests.slice(notificationRequestsStart).every((path) => !path.includes('/views/'));
  await act(async () => notificationButton()!.click());
  await waitFor(() => notificationEvidence !== null);
  const contributedStatus = notificationEvidence;
  const contributedFrame = document.querySelector('iframe')!;
  const notificationIsolation = contributedFrame.getAttribute('sandbox') === 'allow-scripts' && document.querySelector('#plugin-view')!.classList.contains('overlay-modal-surface');
  plugin.enabledIntent = false;
  plugin.lifecycleEpoch++;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => document.querySelector('iframe') === null && notificationButton() === null);
  const disabledRevoked = !contributedFrame.isConnected && document.querySelector('.notification-status-plugins') === null;
  const localErrorAfterDisable = document.querySelector('.notification-status-health')!.textContent!.includes('Local notification store unavailable');
  plugin.enabledIntent = true;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  notificationEvidence = null;
  await waitFor(() => Boolean(notificationButton()));
  await act(async () => notificationButton()!.click());
  await waitFor(() => notificationEvidence !== null);
  await act(async () => notificationChanged?.({ ...notificationSnapshot, syncHealth: { ...notificationSnapshot.syncHealth, libraryId: 'library-b' } }));
  await waitFor(() => document.querySelector('iframe') === null);
  const libraryChangedRevoked = Boolean(notificationButton());
  notificationEvidence = null;
  await act(async () => notificationButton()!.click());
  await waitFor(() => notificationEvidence !== null);
  plugin.permissionRequired = true;
  plugin.lifecycleEpoch++;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => document.querySelector('iframe') === null && document.querySelector('.notification-status-plugins') === null);
  const grantRevoked = document.querySelector('.notification-status-health') !== null;
  plugin.permissionRequired = false;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  notificationEvidence = null;
  await waitFor(() => Boolean(notificationButton()));
  await act(async () => notificationButton()!.click());
  await waitFor(() => notificationEvidence !== null);
  installed = false;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['plugins'] }));
  await waitFor(() => document.querySelector('iframe') === null && document.querySelector('.notification-status-plugins') === null);
  const notifications = { ...notificationAbsent, noMatchingSlot, missingGrantHidden, noHostSyncRequests, noEagerDocument, contributedStatus, notificationIsolation, disabledRevoked, localErrorAfterDisable, libraryChangedRevoked, grantRevoked, uninstalledRevoked: true };
  const privilegedBridge = window.mangooDesktop;
  let entryReads = 0;
  let openedRequest: NotificationPluginOpenRequest | null = null;
  let rejectOpen = false;
  window.mangooDesktop = {
    windowRole: 'notifications', notifications: notificationBridge,
    listNotificationPluginEntries: async () => { entryReads++; return installed && plugin.enabledIntent && !plugin.permissionRequired ? [{ pluginId: plugin.pluginId, viewId: 'notification-status', title: 'Example status' }] : []; },
    openNotificationPluginView: async (request) => { if (rejectOpen) throw new Error('Plugin entry changed'); openedRequest = request; return true; },
  };
  const restrictedRequestStart = requests.length;
  await act(async () => root.render(<QueryClientProvider client={queryClient}><OverlayProvider><NotificationWindow /></OverlayProvider></QueryClientProvider>));
  await act(async () => notificationChanged?.(notificationSnapshot));
  await waitFor(() => entryReads > 0);
  const restrictedEmpty = document.querySelector('.notification-status-plugins') === null;
  installed = true;
  await act(async () => queryClient.invalidateQueries({ queryKey: ['notification-plugin-entries'] }));
  await waitFor(() => Boolean(notificationButton()));
  rejectOpen = true;
  await act(async () => notificationButton()!.click());
  await waitFor(() => document.body.textContent!.includes('Plugin entry changed'));
  const restrictedErrorVisible = document.querySelector('.notification-status-plugins [role="alert"]') !== null;
  rejectOpen = false;
  await act(async () => notificationButton()!.click());
  await waitFor(() => openedRequest !== null);
  const restricted = { empty: restrictedEmpty, noHttp: requests.length === restrictedRequestStart, noFrame: document.querySelector('iframe') === null, noCredentials: !('sessionToken' in window.mangooDesktop) && !('coreUrl' in window.mangooDesktop), openedRequest, errorVisible: restrictedErrorVisible };
  await act(async () => root.render(null));
  const intent: PluginViewWindowIntent = { pluginId: plugin.pluginId, viewId: 'notification-status', libraryUuid: 'library-a', lifecycleEpoch: plugin.lifecycleEpoch, contextRevision: 1 };
  let deliverIntent: ((value: PluginViewWindowIntent) => void) | null = null;
  window.mangooDesktop = { ...privilegedBridge, windowRole: 'plugins', onPluginViewIntent: (callback) => { deliverIntent = callback; callback(intent); return () => { deliverIntent = null; }; } };
  plugin.contributions!.ui!.views.find((view) => view.id === 'main')!.slot = 'plugin-detail';
  notificationEvidence = null;
  await act(async () => root.render(<QueryClientProvider client={queryClient}><OverlayProvider><PluginsWindow /></OverlayProvider></QueryClientProvider>));
  await waitFor(() => notificationEvidence !== null);
  const intentOpened = notificationEvidence;
  await act(async () => document.querySelector<HTMLButtonElement>('#plugin-view .overlay-dialog-actions button')!.click());
  const ordinaryViewButton = () => [...document.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.textContent === 'Example view');
  await waitFor(() => Boolean(ordinaryViewButton()));
  evidence = null;
  await act(async () => ordinaryViewButton()!.click());
  await waitFor(() => evidence !== null);
  const previousFrame = document.querySelector('iframe')!;
  notificationEvidence = null;
  await act(async () => deliverIntent?.(intent));
  await waitFor(() => notificationEvidence !== null);
  const hotIntentReplacesView = !previousFrame.isConnected && document.querySelectorAll('iframe').length === 1 && document.querySelectorAll('#plugin-view').length === 1;
  await act(async () => document.querySelector<HTMLButtonElement>('#plugin-view .overlay-dialog-actions button')!.click());
  await act(async () => deliverIntent?.({ ...intent, contextRevision: 99 }));
  await waitFor(() => document.body.textContent!.includes('插件入口已变化'));
  const staleIntentRejected = document.querySelector('iframe') === null;
  const desktopNotifications = { ...restricted, intentOpened, hotIntentReplacesView, staleIntentRejected };
  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: true, absent, evidence, sandbox, modalShared, flow: initialFlow, keyboard, epochPinned, updatedNotice, changedDocumentRejected, pluginOwnedSetup, notifications, desktopNotifications, parentCsp: document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.getAttribute('content'), revoked: true };
} catch (cause) {
  (window as unknown as { __overlayLifecycleResult: unknown }).__overlayLifecycleResult = { ok: false, error: cause instanceof Error ? cause.stack : String(cause) };
}
