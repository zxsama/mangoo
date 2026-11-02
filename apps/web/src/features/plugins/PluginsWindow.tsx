import { useCallback, useEffect, useRef, useState } from "react";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { useOverlayScrollbars } from "../../app-shell/useOverlayScrollbars";
import { useSecondaryWindowInterfaceStyle } from "../../app-shell/useSecondaryWindowInterfaceStyle";
import { getDesktopBridge } from "../../core/desktop";
import { createCoreClient } from "../../core/client";
import { usePluginViewLauncher } from "./PluginContributionsSlot";
import { useOverlayStack } from "../../design/overlays";
import { pluginUiAvailable } from "./pluginUiBridge";
import { PluginsView } from "./PluginsView";

export function PluginsWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const handleClose = useCallback(async () => {
    const bridge = getDesktopBridge();
    if (bridge?.closePluginsWindow) {
      await bridge.closePluginsWindow();
      return;
    }
    if (bridge?.windowControls) {
      await bridge.windowControls.close();
      return;
    }
    window.close();
  }, []);

  return (
    <SecondaryWindowFrame
      bodyClassName="plugins-window-body"
      onRequestClose={handleClose}
      role="plugins"
      title="Plugins"
      windowClassName="plugins-window"
    >
      <PluginsView />
      <PluginViewIntentHandler />
    </SecondaryWindowFrame>
  );
}

function PluginViewIntentHandler() {
  // PluginsView owns the existing plugin query and cross-window lifecycle refresh.
  const launcher = usePluginViewLauncher();
  const openRef = useRef(launcher.open);
  openRef.current = launcher.open;
  const { closeRoot } = useOverlayStack();
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let generation = 0;
    const unsubscribe = getDesktopBridge()?.onPluginViewIntent?.((intent) => {
      const request = ++generation;
      setError(null);
      const client = createCoreClient();
      void Promise.all([client.getPlugin(intent.pluginId), client.getActiveLibrary()]).then(([plugin, active]) => {
        if (request !== generation) return;
        if (!pluginUiAvailable(plugin) || plugin.lifecycleEpoch !== intent.lifecycleEpoch
          || active.contextRevision !== intent.contextRevision || (active.library?.libraryUuid ?? null) !== intent.libraryUuid
          || !plugin.contributions?.ui?.views.some((view) => view.id === intent.viewId && view.slot === "notification-center")) throw new Error("插件入口已变化，请从通知中心重新打开。");
        closeRoot("plugin-view");
        openRef.current({ plugin, viewId: intent.viewId, libraryUuid: intent.libraryUuid, rootId: null, rootUuid: null, expectedContextRevision: intent.contextRevision });
      }).catch((cause: unknown) => { if (request === generation) setError(cause instanceof Error ? cause.message : String(cause)); });
    });
    return () => { generation++; unsubscribe?.(); };
  }, [closeRoot]);
  return <>{error ? <p role="alert">{error}</p> : null}{launcher.view}</>;
}
