import { CoreApiError } from "../../core-client/src/client";
import type { createCoreClient as createHttpClient } from "../../core-client/src/client";
import type { DesktopFolderPickerRequest } from "../../notification-contract/src/index";
import type { PluginExternalLinkRequest } from "../../core-client/src/generated-contract";
import { CORE_METHODS } from "./core-methods";

export type * from "../../core-client/src/generated-contract";
export type { DesktopFolderPickerRequest } from "../../notification-contract/src/index";
export { CoreApiError, CORE_METHODS };
export type PluginCoreClient = Pick<ReturnType<typeof createHttpClient>, typeof CORE_METHODS[number]>;
export interface PluginUiInit {
  type: "mangoo-plugin-ui:init";
  sessionId: string;
  pluginId: string;
  viewId: string;
  context: { libraryUuid: string | null; rootUuid: string | null; rootId?: number | null };
  theme: { colorScheme: "light" | "dark" };
}
interface Pending { resolve(value: unknown): void; reject(error: Error): void; cleanup(): void }
export interface BridgeWindow {
  parent: { postMessage(message: unknown, targetOrigin: string): void };
  addEventListener(type: "message", listener: (event: MessageEvent) => void): void;
  removeEventListener(type: "message", listener: (event: MessageEvent) => void): void;
}
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const identity = (value: unknown): value is string => typeof value === "string" && value.length > 0 && value.length <= 256;
const hostMethods = new Set(["host.pickFolder", "host.pickNewLibraryPath", "host.revealPath", "host.copyText", "host.openPluginExternalLink", "host.requestOAuthReturnFocus", "host.changed", "host.close"]);
const coreMethods = new Set<string>(CORE_METHODS.map((method) => `core.${method}`));

export function createPluginBridge(target: BridgeWindow, timeoutMs = 30_000) {
  const pending = new Map<string, Pending>();
  let init: PluginUiInit | null = null;
  let stopped = false;
  let serial = 0;
  let resolveReady!: (value: PluginUiInit) => void;
  let rejectReady!: (error: Error) => void;
  const ready = new Promise<PluginUiInit>((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  // Consumers can call readContext before awaiting ready without an unhandled rejection.
  void ready.catch(() => undefined);
  const initTimeout = setTimeout(() => dispose(new Error("Plugin host initialization timed out")), timeoutMs);
  function receive(event: MessageEvent) {
    if (stopped || event.source !== target.parent || !record(event.data)) return;
    const message = event.data;
    if (message.type === "mangoo-plugin-ui:init" && !init) {
      if (!identity(message.sessionId) || !identity(message.pluginId) || !identity(message.viewId) || !record(message.context) || !record(message.theme)) return;
      if (![message.context.libraryUuid, message.context.rootUuid].every((value) => value === null || identity(value))) return;
      const rootId = message.context.rootId ?? null;
      if (rootId !== null && (typeof rootId !== "number" || !Number.isSafeInteger(rootId) || rootId <= 0)) return;
      if (message.theme.colorScheme !== "light" && message.theme.colorScheme !== "dark") return;
      init = { ...message, context: { ...message.context, rootId } } as unknown as PluginUiInit;
      clearTimeout(initTimeout);
      resolveReady(init);
      return;
    }
    if (message.type !== "mangoo-plugin-ui:response" || !init || message.sessionId !== init.sessionId || typeof message.requestId !== "string") return;
    const item = pending.get(message.requestId);
    if (!item) return;
    pending.delete(message.requestId); item.cleanup();
    if (record(message.error) && typeof message.error.message === "string") {
      const error = typeof message.error.status === "number" ? new CoreApiError(message.error.status, message.error.body) : new Error(message.error.message);
      error.message = message.error.message;
      item.reject(error);
    } else if ("error" in message) item.reject(new Error("Malformed plugin host response"));
    else item.resolve(message.result);
  }
  function dispose(reason = new Error("Plugin view closed")) {
    if (stopped) return;
    stopped = true; clearTimeout(initTimeout); target.removeEventListener("message", receive); rejectReady(reason);
    for (const item of pending.values()) { item.cleanup(); item.reject(reason); }
    pending.clear();
  }
  async function call<T>(method: string, args: unknown[] = [], signal?: AbortSignal): Promise<T> {
    if (stopped) throw new Error("Plugin view closed");
    if (!hostMethods.has(method) && !coreMethods.has(method)) throw new Error("Unsupported plugin method");
    if (signal?.aborted) throw new DOMException("Request aborted", "AbortError");
    // Bound calls waiting for initialization as well as posted calls.
    if (pending.size >= 32) throw new Error("Too many pending plugin requests");
    const requestId = String(++serial);
    return new Promise<T>((resolve, reject) => {
      const fail = (error: Error) => { const item = pending.get(requestId); if (!item) return; pending.delete(requestId); item.cleanup(); reject(error); };
      const abort = () => fail(new DOMException("Request aborted", "AbortError"));
      const timer = setTimeout(() => fail(new Error("Plugin request timed out")), timeoutMs);
      pending.set(requestId, { resolve: (value) => resolve(value as T), reject, cleanup: () => { clearTimeout(timer); signal?.removeEventListener("abort", abort); } });
      signal?.addEventListener("abort", abort, { once: true });
      void ready.then((session) => {
        if (!pending.has(requestId) || stopped) return;
        try {
          const encoded = JSON.stringify(args);
          if (encoded.length > 256 * 1024) throw new Error("Plugin request is too large");
          target.parent.postMessage({ type: "mangoo-plugin-ui:request", sessionId: session.sessionId, requestId, method, args: JSON.parse(encoded) }, "*");
        } catch (error) { fail(error instanceof Error ? error : new Error(String(error))); }
      }, fail);
    });
  }
  target.addEventListener("message", receive);
  target.parent.postMessage({ type: "mangoo-plugin-ui:ready" }, "*");
  return { ready, call, dispose, readContext: () => { if (!init) throw new Error("Plugin host is not initialized"); return init; } };
}
export type PluginBridge = ReturnType<typeof createPluginBridge>;
let defaultBridge: PluginBridge | undefined;
export function pluginBridge(): PluginBridge { return defaultBridge ??= createPluginBridge(window); }
export function getPluginContext(): PluginUiInit { return pluginBridge().readContext(); }
export function createCoreClient(bridge: PluginBridge = pluginBridge()): PluginCoreClient {
  const entries = CORE_METHODS.map((method) => [method, (...args: unknown[]) => {
    let signal: AbortSignal | undefined;
    const serializable = args.map((arg) => {
      if (!record(arg) || !("signal" in arg)) return arg;
      const { signal: optionSignal, ...rest } = arg;
      if (optionSignal instanceof AbortSignal) signal = optionSignal;
      return rest;
    });
    return bridge.call(`core.${method}`, serializable, signal);
  }]);
  return Object.fromEntries(entries) as PluginCoreClient;
}
export const host = {
  pickFolder: (request?: DesktopFolderPickerRequest) => pluginBridge().call<string | null>("host.pickFolder", request ? [request] : []),
  pickNewLibraryPath: (defaultName?: string) => pluginBridge().call<string | null>("host.pickNewLibraryPath", defaultName ? [defaultName] : []),
  revealPath: (path: string) => pluginBridge().call<boolean>("host.revealPath", [path]),
  copyText: (text: string) => pluginBridge().call<boolean>("host.copyText", [text]),
  openPluginExternalLink: (request: PluginExternalLinkRequest & { pluginId: string }) => pluginBridge().call<{ ok: true } | { ok: false; error: string }>("host.openPluginExternalLink", [request]),
  requestOAuthReturnFocus: () => pluginBridge().call<boolean>("host.requestOAuthReturnFocus"),
  changed: () => pluginBridge().call<void>("host.changed"),
  close: () => pluginBridge().call<void>("host.close"),
};
