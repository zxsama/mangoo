import React from "react";
import ReactDOM from "react-dom/client";
import { getDesktopBridge, notifyDesktopShellReady } from "./core/desktop";
import "./styles.css";

// Secondary OS windows load this same bundle with a role + matching hash route.
// Hash detection also gives browser builds a safe named-window fallback.
const windowRole = getDesktopBridge()?.windowRole;
const isSettingsWindow =
  window.location.hash.startsWith("#/settings") || windowRole === "settings";
const isPluginsWindow =
  window.location.hash.startsWith("#/plugins") || windowRole === "plugins";

if (isPluginsWindow) {
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
        <App />
      </React.StrictMode>
    );
  });
}
