import { useCallback, useEffect, useState } from "react";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { getDesktopBridge } from "../../core/desktop";
import { broadcastPreference } from "../../core/preferenceSync";
import { useLibraryData } from "../../core/useLibraryData";
import { useInterfaceStyle } from "../../design/liquid-glass";
import {
  readStoredLibraryGridPreferences,
  storeLibraryGridPreferences,
  type LibraryGridPreferences
} from "../media-grid/gridPreferences";
import {
  readStoredPreviewPreferences,
  storePreviewPreferences,
  type PreviewPreferences
} from "../preview/previewPreferences";
import { SettingsPanels } from "./settingsPanels";

// The renderer entry for the SEPARATE settings OS window (a second frameless
// BrowserWindow at #/settings). It owns its own preference state (loaded from
// localStorage), applies changes locally, persists them, AND broadcasts them so
// the main window updates live — the two windows share this app's origin, so a
// BroadcastChannel reaches across them without any per-setting IPC.
export function SettingsWindow() {
  const interfaceStyle = useInterfaceStyle();
  const [gridPreferences, setGridPreferences] = useState<LibraryGridPreferences>(() =>
    readStoredLibraryGridPreferences()
  );
  const [previewPreferences, setPreviewPreferences] = useState<PreviewPreferences>(() =>
    readStoredPreviewPreferences()
  );

  // The thumbnails tab needs cache stats + generation actions, which live on the
  // library hook. Running it here is fine — the settings window is occasional and
  // gets its own Core client from the preload bridge.
  const library = useLibraryData({
    persistentThumbnailCacheAutoRefresh: previewPreferences.persistentThumbnailCacheAutoRefresh,
    thumbnailGenerationMode: previewPreferences.thumbnailGenerationMode
  });

  const handleClose = useCallback(async () => {
    const bridge = getDesktopBridge();
    if (bridge?.closeSettingsWindow) {
      await bridge.closeSettingsWindow();
      return;
    }
    if (bridge?.windowControls) {
      await bridge.windowControls.close();
      return;
    }
    window.close();
  }, []);

  const handleGridChange = useCallback((patch: Partial<LibraryGridPreferences>) => {
    setGridPreferences((current) => {
      const next = { ...current, ...patch };
      storeLibraryGridPreferences(next);
      broadcastPreference("grid", next);
      return next;
    });
  }, []);

  const handlePreviewChange = useCallback((patch: Partial<PreviewPreferences>) => {
    setPreviewPreferences((current) => {
      const next = { ...current, ...patch };
      storePreviewPreferences(next);
      broadcastPreference("preview", next);
      return next;
    });
  }, []);

  // Interface-style changes are already persisted by useInterfaceStyle; broadcast
  // the value on change so the main window's live style updates too.
  useEffect(() => {
    broadcastPreference("interfaceStyle", interfaceStyle.value);
  }, [interfaceStyle.value]);

  return (
    <SecondaryWindowFrame
      bodyClassName="settings-window-body"
      onRequestClose={handleClose}
      title="偏好设置"
      windowClassName="settings-window"
    >
      <SettingsPanels
        gridPreferences={gridPreferences}
        interfaceStyle={interfaceStyle}
        library={library}
        onGridPreferencesChange={handleGridChange}
        onPreviewPreferencesChange={handlePreviewChange}
        previewPreferences={previewPreferences}
      />
    </SecondaryWindowFrame>
  );
}
