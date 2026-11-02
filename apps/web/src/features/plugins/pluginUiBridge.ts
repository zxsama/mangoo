import { CoreApiError, type HostPluginProjection } from "@mangoo/core-client";
import { pluginUiCoreMethods, type PluginUiCoreClient, type PluginUiCoreMethod } from "./pluginUiCoreMethods.ts";
import { isBoundedJson, nullable, object, oneOf, optional, record, text, tuple } from "./pluginUiValidation.ts";
import type * as desktop from "../../core/desktop";

export interface PluginUiContext { libraryUuid: string | null; rootId: number | null; rootUuid: string | null; }
export interface PluginUiTheme { colorScheme: "light" | "dark"; }
export type PluginUiHost = Pick<typeof desktop, "pickNativeFolder" | "pickNewLibraryPath" | "revealPath" | "copyText" | "openPluginExternalLink" | "requestOAuthReturnFocus">;
type Operation = "core-api" | "pick-folder" | "reveal-path" | "clipboard-write" | "external-link";
export const PLUGIN_UI_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; base-uri 'none'; form-action 'none'";
export const pluginUiDocument = (document: string): string => `<!doctype html><meta http-equiv="Content-Security-Policy" content="${PLUGIN_UI_CSP}"><meta name="referrer" content="no-referrer">${document}`;

export function pluginUiAvailable(plugin: HostPluginProjection): boolean {
  return Boolean(plugin.contributions?.ui) && plugin.enabledIntent && !plugin.permissionRequired
    && !["disabled", "stopping", "incompatible", "permission-required"].includes(plugin.runtimeState);
}
function granted(plugin: HostPluginProjection, operation: Operation): boolean {
  return plugin.grantedPermissions.some((permission) => permission.kind === "host-ui" && permission.operations.includes(operation));
}
const authMethods = new Set<PluginUiCoreMethod>(["getPlugin", "getPluginConfiguration", "beginPluginAuth", "completePluginAuth", "cancelPluginAuth", "signOutPlugin", "resolvePluginExternalLink", "planPluginConfiguration", "commitPluginConfiguration"]);
const globalMethods = new Set<PluginUiCoreMethod>(["getActiveLibrary", "listLibraries", "getSyncStatus", "listRemoteLibraries", "listRemoteLibraryRoots", "preflightLibrary", "listRoots", "preflightSyncRestore", "startSyncRestore", "getSyncRestore", "pauseSyncRestore", "resumeSyncRestore", "cancelSyncRestore"]);
const restoreMethods = new Set<PluginUiCoreMethod>(["getSyncRestore", "pauseSyncRestore", "resumeSyncRestore", "cancelSyncRestore"]);
const mutableMethod = (method: string) => !/^(get|list|preflight|plan|resolvePluginExternalLink)/u.test(method);

/** Compare spelling only. Never resolve segments or turn ordinary paths into device paths. */
function matchesHostPath(requested: string, canonical: string): boolean {
  const drive = /^[A-Za-z]:[\\/]/u;
  const device = /^[\\/]{2}[?.][\\/]/u;
  if (device.test(canonical) || device.test(requested)) return requested === canonical;
  const windows = drive.test(canonical) || canonical.startsWith("\\\\");
  if (!windows) return requested === canonical;
  if (drive.test(canonical) ? !drive.test(requested) : !/^[\\/]{2}[^\\/]+[\\/][^\\/]+/u.test(requested)) return false;
  const key = (path: string) => path.replace(/\//gu, "\\").replace(/\\+$/u, "").toLowerCase();
  return key(requested) === key(canonical);
}

export interface PluginUiBridgeOptions {
  client: PluginUiCoreClient;
  plugin: HostPluginProjection;
  viewId: string;
  context: PluginUiContext;
  contextRevision: number;
  theme: PluginUiTheme;
  sessionId: string;
  source: object;
  post: (message: unknown) => void;
  abort: () => void;
  host: PluginUiHost;
  changed: () => void | Promise<void>;
  close: () => void;
}

/** A single live iframe owns this session. Nothing from its document enters renderer execution. */
export function createPluginUiBridge(o: PluginUiBridgeOptions) {
  const methods = pluginUiCoreMethods(o.client, o.contextRevision);
  const pending = new Set<string>();
  let requestHighWater = 0;
  const pickedPaths = new Set<string>();
  let live = true;
  let ready = false;
  let mutationsInFlight = 0;
  let admissionRevoked = false;
  const assertLive = () => { if (!live) throw new Error("Plugin view session ended"); };
  const revoke = () => { live = false; pending.clear(); o.abort(); };
  const post = (message: unknown) => { if (live) o.post(message); };
  const deny = (): never => { throw new Error("Plugin view operation is not permitted"); };

  async function check(operation: Operation): Promise<HostPluginProjection> {
    assertLive();
    const active = await o.client.getActiveLibrary();
    assertLive();
    if (active.contextRevision !== o.contextRevision) { revoke(); o.close(); throw new Error("Library context changed"); }
    // Plugin admission is last: ownership reads may await I/O while disable/re-enable advances its epoch.
    const plugin = await o.client.getPlugin(o.plugin.pluginId);
    assertLive();
    if (!pluginUiAvailable(plugin) || plugin.pluginId !== o.plugin.pluginId || plugin.selectedVersion !== o.plugin.selectedVersion
      || plugin.lifecycleEpoch !== o.plugin.lifecycleEpoch || plugin.accountUuid !== o.plugin.accountUuid
      || plugin.contributions?.ui?.sha256 !== o.plugin.contributions?.ui?.sha256
      || !plugin.contributions?.ui?.views.some((view) => view.id === o.viewId)) { revoke(); o.close(); throw new Error("Plugin view is no longer available"); }
    if (!granted(plugin, operation)) deny();
    return plugin;
  }

  async function binding(plugin: HostPluginProjection, id: unknown, allowLocal = false) {
    if (typeof id !== "string" || (o.context.libraryUuid && id !== o.context.libraryUuid)) deny();
    const status = (await o.client.getSyncStatus()).libraries.find((item) => item.libraryUuid === id);
    assertLive();
    if (status?.providerId && status.providerId !== plugin.pluginId) deny();
    if (status?.accountUuid && status.accountUuid !== plugin.accountUuid) deny();
    if (!allowLocal && (!status?.providerId || !plugin.accountUuid || status.accountUuid !== plugin.accountUuid)) deny();
    if (allowLocal && !status) {
      const library = (await o.client.listLibraries()).items.find((item) => item.libraryUuid === id);
      if (!library || library.bindingState !== "local-only") deny();
    }
  }

  function identities(value: unknown, plugin: HostPluginProjection) {
    if (Array.isArray(value)) { value.forEach((child) => identities(child, plugin)); return; }
    if (!record(value)) return;
    for (const [key, child] of Object.entries(value)) {
      if ((key === "providerId" || key === "pluginId") && child !== plugin.pluginId) deny();
      if (key === "accountUuid" && (!plugin.accountUuid || child !== plugin.accountUuid)) deny();
      identities(child, plugin);
    }
  }

  async function core(method: PluginUiCoreMethod, args: unknown[], plugin: HostPluginProjection) {
    if (!methods[method].validate(args)) throw new Error("Invalid plugin method arguments");
    identities(args, plugin);
    if (authMethods.has(method)) { if (args[0] !== plugin.pluginId) deny(); }
    else if (!globalMethods.has(method)) {
      await binding(plugin, args[0], ["preflightLibrarySync", "startLibrarySyncEnable", "preflightLibrarySyncReconnect", "commitLibrarySyncReconnect"].includes(method));
      if (o.context.rootUuid && typeof args[1] === "string" && args[1] !== o.context.rootUuid) deny();
    }
    if (method === "listRoots") {
      const active = await o.client.getActiveLibrary();
      await binding(plugin, active.library?.libraryUuid, true);
    }
    if (method === "listRemoteLibraryRoots" && args[0] !== plugin.accountUuid) deny();
    if (restoreMethods.has(method)) {
      const restore = await o.client.getSyncRestore(String(args[0]));
      if (!plugin.accountUuid || restore.accountUuid !== plugin.accountUuid) deny();
    }
    if (method === "preflightSyncRestore" || method === "startSyncRestore") {
      const body = args[0];
      if (!record(body) || !Array.isArray(body.items)) return deny();
      const snapshot = await o.client.getSyncStatus();
      for (const item of body.items) {
        if (!record(item)) deny();
        const existing = snapshot.libraries.find((library) => library.libraryUuid === item.libraryUuid);
        if (existing?.providerId && (existing.providerId !== plugin.pluginId || existing.accountUuid !== plugin.accountUuid)) deny();
      }
    }
    if (method === "resolvePluginExternalLink" && !granted(plugin, "external-link")) deny();
    await check("core-api");
    if (method === "resolvePluginExternalLink") await check("external-link");
    const result = await methods[method].invoke(args);
    assertLive();
    if (method === "getSyncStatus") {
      // Decode using the original typed endpoint, never expose other plugin accounts or bindings.
      const snapshot = result as Awaited<ReturnType<PluginUiCoreClient["getSyncStatus"]>>;
      return { ...snapshot, providers: snapshot.providers.filter((p) => p.pluginId === plugin.pluginId && p.accountUuid === plugin.accountUuid), libraries: snapshot.libraries.filter((l) => (!o.context.libraryUuid || l.libraryUuid === o.context.libraryUuid) && ((!l.providerId && !l.accountUuid) || (l.providerId === plugin.pluginId && l.accountUuid === plugin.accountUuid))) };
    }
    if (method === "listRemoteLibraries") {
      const response = result as Awaited<ReturnType<PluginUiCoreClient["listRemoteLibraries"]>>;
      const items = response.items.filter((item) => item.accountUuid === plugin.accountUuid);
      return { ...response, items, totalCount: items.length };
    }
    return result;
  }

  async function host(method: string, args: unknown[]) {
    const operation: Operation = method === "host.pickFolder" || method === "host.pickNewLibraryPath" ? "pick-folder" : method === "host.revealPath" ? "reveal-path" : method === "host.copyText" ? "clipboard-write" : method === "host.openPluginExternalLink" || method === "host.requestOAuthReturnFocus" ? "external-link" : "core-api";
    const plugin = await check(operation);
    switch (method) {
      case "host.pickFolder": {
        const rule = optional(object({ libraryUuid: text, rootUuid: nullable(text), mode: oneOf("enroll-existing", "materialize-remote", "relocate", "rebind-existing"), suggestedPath: nullable(text), dialogTitle: text }));
        if (!tuple([rule], args)) deny();
        const request = args[0] as Parameters<PluginUiHost["pickNativeFolder"]>[0];
        if (request && request.mode === "materialize-remote") {
          const remote = await o.client.listRemoteLibraries();
          if (!remote.items.some((item) => item.accountUuid === plugin.accountUuid && item.libraryUuid === request.libraryUuid)) await binding(plugin, request.libraryUuid);
        } else if (request) await binding(plugin, request.libraryUuid, true);
        await check(operation);
        const path = await o.host.pickNativeFolder(request);
        assertLive();
        if (path) pickedPaths.add(path);
        return path;
      }
      case "host.pickNewLibraryPath": {
        if (!tuple([optional(text)], args)) deny();
        await check(operation);
        const path = await o.host.pickNewLibraryPath(args[0] as string | undefined);
        assertLive();
        if (path) pickedPaths.add(path);
        return path;
      }
      case "host.revealPath": {
        if (!tuple([text], args)) deny();
        const path = String(args[0]);
        const candidates = new Set([...pickedPaths].filter((canonical) => matchesHostPath(path, canonical)));
        if (!candidates.size) {
          if (o.context.rootId !== null) {
            const active = await o.client.getActiveLibrary();
            if (active.library?.libraryUuid !== o.context.libraryUuid) deny();
            const roots = await o.client.listRoots();
            if (roots.items.filter((root) => root.id === o.context.rootId && matchesHostPath(path, root.path)).length !== 1) deny();
          }
          const statuses = (await o.client.getSyncStatus()).libraries.filter((item) => item.providerId === plugin.pluginId && item.accountUuid === plugin.accountUuid && (!o.context.libraryUuid || item.libraryUuid === o.context.libraryUuid));
          for (const status of statuses) {
            const roots = await o.client.listSyncFolders(status.libraryUuid);
            for (const root of roots.items) {
              const canonical = root.binding?.localPath;
              if (canonical && matchesHostPath(path, canonical) && (!o.context.rootUuid || root.descriptor.rootUuid === o.context.rootUuid)) candidates.add(canonical);
            }
          }
          if (o.context.rootId === null && !o.context.rootUuid) {
            const libraries = await o.client.listLibraries();
            for (const item of libraries.items) {
              if (item.active && matchesHostPath(path, item.databasePath) && (!o.context.libraryUuid || item.libraryUuid === o.context.libraryUuid)) candidates.add(item.databasePath);
            }
          }
        }
        // Equivalent but differently spelled Host candidates are ambiguous; never select the first.
        if (candidates.size !== 1) deny();
        await check(operation);
        return o.host.revealPath([...candidates][0]!);
      }
      case "host.copyText": if (!tuple([(v) => typeof v === "string" && v.length <= 262144], args)) deny(); await check(operation); return o.host.copyText(String(args[0]));
      case "host.openPluginExternalLink": {
        if (!tuple([object({ pluginId: text, kind: oneOf("authorization", "help"), id: text })], args)) deny();
        const request = args[0] as Parameters<PluginUiHost["openPluginExternalLink"]>[0];
        if (request.pluginId !== plugin.pluginId) deny();
        await check(operation);
        return o.host.openPluginExternalLink(request);
      }
      case "host.requestOAuthReturnFocus": if (args.length) deny(); await check(operation); return o.host.requestOAuthReturnFocus();
      case "host.changed": if (args.length) deny(); await check(operation); return null;
      case "host.close": if (args.length) deny(); await check(operation); o.close(); revoke(); return null;
      default: return deny();
    }
  }

  async function receive(event: { source: object | null; data: unknown }) {
    if (!live || event.source !== o.source || !isBoundedJson(event.data) || !record(event.data)) return;
    const message = event.data;
    if (message.type === "mangoo-plugin-ui:ready") {
      if (ready) return;
      ready = true;
      post({ type: "mangoo-plugin-ui:init", sessionId: o.sessionId, pluginId: o.plugin.pluginId, viewId: o.viewId, context: o.context, theme: o.theme });
      return;
    }
    if (!ready || message.type !== "mangoo-plugin-ui:request" || message.sessionId !== o.sessionId || typeof message.requestId !== "string" || !/^[1-9][0-9]{0,15}$/u.test(message.requestId) || typeof message.method !== "string" || !Array.isArray(message.args)) return;
    const { requestId, method, args } = message;
    const serial = Number(requestId);
    if (!Number.isSafeInteger(serial) || serial <= requestHighWater) return;
    requestHighWater = serial;
    const envelope = { type: "mangoo-plugin-ui:response", sessionId: o.sessionId, requestId };
    if (pending.size >= 32) { post({ ...envelope, error: { message: "Plugin view request limit exceeded", status: 429 } }); return; }
    pending.add(requestId);
    const mutation = method.startsWith("core.") && Object.hasOwn(methods, method.slice(5)) && mutableMethod(method.slice(5));
    if (mutation) mutationsInFlight++;
    try {
      let result: unknown;
      if (method.startsWith("core.") && Object.hasOwn(methods, method.slice(5))) {
        const plugin = await check("core-api");
        result = await core(method.slice(5) as PluginUiCoreMethod, args, plugin);
      } else result = await host(method, args);
      if (!isBoundedJson(result ?? null, 8 * 1024 * 1024)) throw new Error("Plugin response exceeds document budget");
      post({ ...envelope, result: result ?? null });
      // The successful operation is acknowledged before refresh can revoke its old session.
      if (method === "host.changed" || mutation) {
        try { await o.changed(); } catch { /* A refresh failure cannot turn an acknowledged mutation into a second result. */ }
      }
    } catch (cause) {
      const error = cause instanceof CoreApiError ? { message: cause.message, status: cause.status, body: isBoundedJson(cause.body) ? cause.body : null } : { message: cause instanceof Error ? cause.message : "Plugin request failed" };
      post({ ...envelope, error });
      if (cause instanceof CoreApiError && record(cause.body) && (cause.body.code === "plugin-ui-session-changed" || cause.body.code === "plugin-ui-target-changed")) admissionRevoked = true;
    } finally {
      pending.delete(requestId);
      if (mutation) mutationsInFlight--;
      // Let an already successful self-mutation publish its Host notice before a concurrent poll closes it.
      if (live && admissionRevoked && mutationsInFlight === 0) { revoke(); o.close(); }
    }
  }
  return { receive, revoke, get live() { return live; } };
}
