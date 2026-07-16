import { useCallback } from "react";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { getDesktopBridge } from "../../core/desktop";
import { PluginsView } from "./PluginsView";

export function PluginsWindow() {
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
      title="Plugins"
      windowClassName="plugins-window"
    >
      <PluginsView />
    </SecondaryWindowFrame>
  );
}
