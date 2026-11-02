import { useEffect, useRef, useState, type RefObject } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { createCoreClient as createTransportClient, type HostPluginProjection, type HostPluginViewDocument } from "@mangoo/core-client";
import { createCoreClient, getCoreClientConfig } from "../../core/client";
import * as desktop from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayInlineStatus } from "../../design/overlays";
import { broadcastPluginChange, invalidatePluginQueries } from "./pluginQuerySync";
import { createPluginUiBridge, pluginUiDocument, type PluginUiContext } from "./pluginUiBridge";

export interface PluginViewSelection extends PluginUiContext { plugin: HostPluginProjection; viewId: string; expectedContextRevision?: number; }

export function PluginView({ selection, onClose, onStateChanged, restoreFocusRef }: { selection: PluginViewSelection; onClose: () => void; onStateChanged: () => void; restoreFocusRef?: RefObject<HTMLElement | null> }) {
  const queryClient = useQueryClient();
  const iframe = useRef<HTMLIFrameElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const onStateChangedRef = useRef(onStateChanged);
  onStateChangedRef.current = onStateChanged;
  const [document, setDocument] = useState<HostPluginViewDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const bridgeRef = useRef<ReturnType<typeof createPluginUiBridge> | null>(null);
  const loadCount = useRef(0);
  const title = selection.plugin.contributions?.ui?.views.find((view) => view.id === selection.viewId)?.title ?? selection.plugin.name;

  useEffect(() => {
    const controller = new AbortController();
    setDocument(null); setError(null); loadCount.current = 0;
    const client = createTransportClient({ ...getCoreClientConfig(), signal: controller.signal });
    let sessionClient: ReturnType<typeof createTransportClient> | null = null;
    let disposed = false;
    let bridge: ReturnType<typeof createPluginUiBridge> | null = null;
    let contextRevision = 0;
    const receive = (event: MessageEvent<unknown>) => {
      if (!iframe.current?.contentWindow || event.source !== iframe.current.contentWindow || disposed || !sessionClient) return;
      if (!bridge) {
        bridge = createPluginUiBridge({ client: sessionClient, plugin: selection.plugin, viewId: selection.viewId, context: { libraryUuid: selection.libraryUuid, rootId: selection.rootId, rootUuid: selection.rootUuid }, contextRevision, theme: { colorScheme: getComputedStyle(window.document.documentElement).colorScheme === "light" ? "light" : "dark" }, sessionId: crypto.randomUUID(), source: iframe.current.contentWindow, post: (message) => iframe.current?.contentWindow?.postMessage(message, "*"), abort: () => controller.abort(), host: desktop,
          changed: async () => {
            const current = await client.getPlugin(selection.plugin.pluginId);
            if (!disposed && (current.lifecycleEpoch !== selection.plugin.lifecycleEpoch || current.accountUuid !== selection.plugin.accountUuid)) { bridge?.revoke(); onStateChangedRef.current(); }
            await invalidatePluginQueries(queryClient); broadcastPluginChange();
          }, close: () => onCloseRef.current() });
        bridgeRef.current = bridge;
      }
      void bridge.receive(event).catch((cause: unknown) => { if (!disposed) setError(cause instanceof Error ? cause.message : String(cause)); });
    };
    window.addEventListener("message", receive);
    void Promise.all([client.getPluginViewDocument(selection.plugin.pluginId, selection.viewId), client.getActiveLibrary()]).then(([view, active]) => {
      if (disposed) return;
      if (view.pluginId !== selection.plugin.pluginId || view.version !== selection.plugin.selectedVersion || view.viewId !== selection.viewId || view.lifecycleEpoch !== selection.plugin.lifecycleEpoch) throw new Error("Plugin view changed while loading");
      if (selection.expectedContextRevision !== undefined && active.contextRevision !== selection.expectedContextRevision) throw new Error("Plugin view context changed while loading");
      sessionClient = createTransportClient({ ...getCoreClientConfig(), signal: controller.signal, pluginUiSession: { pluginId: view.pluginId, lifecycleEpoch: view.lifecycleEpoch } });
      contextRevision = active.contextRevision;
      setDocument(view);
    }).catch((cause: unknown) => { if (!disposed) setError(cause instanceof Error ? cause.message : String(cause)); });
    const unsubscribe = desktop.subscribeLibraryPhase(() => {
      void createCoreClient().getActiveLibrary().then((active) => {
        if (!disposed && bridge && active.contextRevision !== contextRevision) { bridge.revoke(); onCloseRef.current(); }
      }).catch(() => { /* A subsequent dispatch rechecks the authoritative context. */ });
    });
    return () => { disposed = true; controller.abort(); bridge?.revoke(); bridgeRef.current = null; unsubscribe(); window.removeEventListener("message", receive); };
  }, [selection.plugin.pluginId, selection.plugin.selectedVersion, selection.plugin.lifecycleEpoch, selection.plugin.contributions?.ui?.sha256, selection.viewId, selection.libraryUuid, selection.rootId, selection.rootUuid, selection.expectedContextRevision, queryClient]);

  return <ModalSurface ariaLabelledBy="plugin-view-title" className="plugin-view-modal" id="plugin-view" initialFocusRef={closeRef} onDismiss={onClose} open restoreFocusRef={restoreFocusRef} role="dialog" rootId="plugin-view">
    <OverlayDialogHeader title={title} titleId="plugin-view-title" />
    <OverlayDialogBody className="plugin-view-body">
      {error ? <OverlayInlineStatus tone="error">{error}</OverlayInlineStatus> : document ? <iframe className="plugin-view-frame" ref={iframe} title={title} tabIndex={0} sandbox="allow-scripts" referrerPolicy="no-referrer" srcDoc={pluginUiDocument(document.document)} onLoad={() => { if (++loadCount.current > 1) { bridgeRef.current?.revoke(); onCloseRef.current(); } }} /> : <p role="status">正在加载插件视图…</p>}
    </OverlayDialogBody>
    <OverlayDialogActions><LiquidGlassButton onClick={onClose} ref={closeRef} type="button">关闭</LiquidGlassButton></OverlayDialogActions>
  </ModalSurface>;
}
