import { useCallback, useRef, useState, type RefCallback } from "react";
import { useOverlayStack } from "../design/overlays";

export interface ShellNotificationOverlayWiring {
  closeNotifications: () => void;
  notificationAnchor: HTMLButtonElement | null;
  notificationAutoFocus: boolean;
  notificationButtonRef: RefCallback<HTMLButtonElement>;
  notificationsOpen: boolean;
  onToggleNotifications: (autoFocus: boolean) => void;
}

export function useShellNotificationOverlay(): ShellNotificationOverlayWiring {
  const overlays = useOverlayStack();
  const [notificationAnchor, setNotificationAnchor] = useState<HTMLButtonElement | null>(null);
  const notificationAutoFocusRef = useRef(false);
  const notificationsOpen = overlays.isRootOpen("notifications");

  const onToggleNotifications = useCallback((autoFocus: boolean) => {
    notificationAutoFocusRef.current = !notificationsOpen && autoFocus;
    if (notificationsOpen) overlays.closeRoot("notifications");
    else overlays.openRoot("notifications");
  }, [notificationsOpen, overlays]);

  const closeNotifications = useCallback(() => {
    overlays.closeRoot("notifications");
  }, [overlays]);

  return {
    closeNotifications,
    notificationAnchor,
    notificationAutoFocus: notificationAutoFocusRef.current,
    notificationButtonRef: setNotificationAnchor,
    notificationsOpen,
    onToggleNotifications
  };
}
