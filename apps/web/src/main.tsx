import React from "react";
import ReactDOM from "react-dom/client";
import { getDesktopBridge, notifyDesktopShellReady } from "./core/desktop";
import "./styles.css";

// The separate settings OS window loads this same bundle at #/settings; render
// the standalone settings surface instead of the full app shell there.
const isSettingsWindow =
  window.location.hash.startsWith("#/settings") || getDesktopBridge()?.windowRole === "settings";

if (isSettingsWindow) {
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
