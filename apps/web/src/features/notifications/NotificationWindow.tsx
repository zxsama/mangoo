import { useCallback } from "react";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { useSecondaryWindowInterfaceStyle } from "../../app-shell/useSecondaryWindowInterfaceStyle";
import { getDesktopBridge } from "../../core/desktop";

// Task 13 replaces this shell with the full Status/History/Logs workspace.
export function NotificationWindow() {
  useSecondaryWindowInterfaceStyle();
  const handleClose = useCallback(async () => {
    const bridge = getDesktopBridge();
    if (bridge?.closeNotificationsWindow) {
      await bridge.closeNotificationsWindow();
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
      onRequestClose={handleClose}
      role="notifications"
      title="通知中心"
      windowClassName="notification-window"
    >
      <section aria-label="通知中心" />
    </SecondaryWindowFrame>
  );
}
