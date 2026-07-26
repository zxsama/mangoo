import React from "react";
import ReactDOM from "react-dom/client";
import { getDesktopBridge, notifyDesktopShellReady } from "./core/desktop";
import { applyInterfaceStyleVariables, readInterfaceStyle } from "./design/liquid-glass";
import { OverlayProvider } from "./design/overlays";
import "./styles.css";
import "./design/overlays/overlay.css";

// Apply persisted material variables before any role-specific dynamic import or
// first paint. Hooks keep them current after mount; this closes the blank default
// CSS frame that used to appear before Settings/Plugins initialized.
applyInterfaceStyleVariables(readInterfaceStyle());

// Secondary OS windows load this same bundle with a role + matching hash route.
// Hash detection also gives browser builds a safe named-window fallback.
const windowRole = getDesktopBridge()?.windowRole;
const isSettingsWindow =
  window.location.hash.startsWith("#/settings") || windowRole === "settings";
const isPluginsWindow =
  window.location.hash.startsWith("#/plugins") || windowRole === "plugins";
const isNotificationsWindow =
  window.location.hash.startsWith("#/notifications") || windowRole === "notifications";

if (isNotificationsWindow) {
  void import("./features/notifications/NotificationWindow").then(({ NotificationWindow }) => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <NotificationWindow />
      </React.StrictMode>
    );
  });
} else if (isPluginsWindow) {
  void import("./features/plugins/PluginsWindow").then(({ PluginsWindow }) => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <PluginsWindow />
      </React.StrictMode>
    );
  });
} else if (isSettingsWindow) {
  void import("./features/settings/SettingsWindow").then(({ SettingsWindow }) => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <SettingsWindow />
      </React.StrictMode>
    );
  });
} else {
  void notifyDesktopShellReady();
  void import("./app/App").then(({ App }) => {
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <OverlayProvider>
          <App />
        </OverlayProvider>
      </React.StrictMode>
    );
    preloadSecondaryWindowRenderers();
  });
}

function preloadSecondaryWindowRenderers() {
  const preload = () => {
    void import("./features/plugins/PluginsWindow");
    void import("./features/settings/SettingsWindow");
    void import("./features/notifications/NotificationWindow");
  };
  if (typeof window.requestIdleCallback === "function") {
    window.requestIdleCallback(preload, { timeout: 1200 });
  } else {
    window.setTimeout(preload, 0);
  }
}
