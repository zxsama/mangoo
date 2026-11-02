import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { PluginUiSlot } from "@mangoo/core-client";
import { createCoreClient } from "../../core/client";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, useOverlayStack } from "../../design/overlays";
import { PluginView, type PluginViewSelection } from "./PluginView";
import { pluginUiAvailable } from "./pluginUiBridge";
import { usePluginQuerySync } from "./pluginQuerySync";

export function usePluginContributionViews(slot: PluginUiSlot, pluginId?: string) {
  const queryClient = useQueryClient();
  usePluginQuerySync(queryClient);
  const query = useQuery({ queryKey: ["plugins"], queryFn: () => createCoreClient().listPlugins(), refetchInterval: 30000 });
  return (query.data?.items ?? []).filter((plugin) => (!pluginId || plugin.pluginId === pluginId) && pluginUiAvailable(plugin)).flatMap((plugin) => (plugin.contributions?.ui?.views ?? []).filter((view) => view.slot === slot).map((view) => ({ plugin, view })));
}

export function usePluginViewLauncher() {
  const overlays = useOverlayStack();
  const [selection, setSelection] = useState<PluginViewSelection | null>(null);
  const [updated, setUpdated] = useState(false);
  const noticeCloseRef = useRef<HTMLButtonElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const close = useCallback(() => { setSelection(null); setUpdated(false); overlays.closeRoot("plugin-view"); }, [overlays]);
  const stateChanged = useCallback(() => { setSelection(null); setUpdated(true); }, []);
  const open = useCallback((next: PluginViewSelection, source?: HTMLElement | null) => {
    restoreFocusRef.current = source ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    overlays.openRoot("plugin-view");
    setUpdated(false);
    setSelection({ ...next, plugin: structuredClone(next.plugin) });
  }, [overlays]);
  const queryClient = useQueryClient();
  useEffect(() => queryClient.getQueryCache().subscribe(() => {
    if (!selection) return;
    const state = queryClient.getQueryData<Awaited<ReturnType<ReturnType<typeof createCoreClient>["listPlugins"]>>>(["plugins"]);
    const plugin = state?.items.find((p) => p.pluginId === selection.plugin.pluginId);
    if (state && (!plugin || !pluginUiAvailable(plugin) || plugin.selectedVersion !== selection.plugin.selectedVersion || plugin.lifecycleEpoch !== selection.plugin.lifecycleEpoch || plugin.contributions?.ui?.sha256 !== selection.plugin.contributions?.ui?.sha256)) close();
  }), [selection, queryClient, close]);
  return { open, view: <>{selection ? <PluginView key={`${selection.plugin.pluginId}:${selection.viewId}`} selection={selection} onClose={close} onStateChanged={stateChanged} restoreFocusRef={restoreFocusRef} /> : null}{updated ? <ModalSurface ariaLabelledBy="plugin-view-updated-title" id="plugin-view-updated" initialFocusRef={noticeCloseRef} onDismiss={close} open restoreFocusRef={restoreFocusRef} role="dialog" rootId="plugin-view"><OverlayDialogHeader title="插件状态已更新" titleId="plugin-view-updated-title" /><OverlayDialogBody><p role="status">插件状态已更新，请重新打开视图继续。</p></OverlayDialogBody><OverlayDialogActions><LiquidGlassButton onClick={close} ref={noticeCloseRef} type="button">关闭</LiquidGlassButton></OverlayDialogActions></ModalSurface> : null}</> };
}

export function PluginContributionsSlot({ slot, pluginId, libraryUuid = null, rootId = null, rootUuid = null }: { slot: PluginUiSlot; pluginId?: string; libraryUuid?: string | null; rootId?: number | null; rootUuid?: string | null }) {
  const views = usePluginContributionViews(slot, pluginId);
  const launcher = usePluginViewLauncher();
  return <>{views.map(({ plugin, view }) => <LiquidGlassButton key={`${plugin.pluginId}:${view.id}`} type="button" onClick={(event) => { event.stopPropagation(); launcher.open({ plugin, viewId: view.id, libraryUuid, rootId, rootUuid }, event.currentTarget); }}>{view.title}</LiquidGlassButton>)}{launcher.view}</>;
}
