import { X } from "lucide-react";
import { useCallback, useEffect, useState, type PointerEvent as ReactPointerEvent } from "react";
import { getDesktopBridge } from "../../core/desktop";
import { broadcastPreference } from "../../core/preferenceSync";
import { useLibraryData } from "../../core/useLibraryData";
import { LiquidGlassLayer, useInterfaceStyle } from "../../design/liquid-glass";
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

  useEffect(() => {
    function handleKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        void getDesktopBridge()?.closeSettingsWindow?.();
      }
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, []);

  return (
    <LiquidGlassLayer>
      <div className="settings-window">
        <SettingsTitlebar />
        <div className="settings-window-body">
          <SettingsPanels
            gridPreferences={gridPreferences}
            interfaceStyle={interfaceStyle}
            library={library}
            onGridPreferencesChange={handleGridChange}
            onPreviewPreferencesChange={handlePreviewChange}
            previewPreferences={previewPreferences}
          />
        </div>
      </div>
    </LiquidGlassLayer>
  );
}

// The frameless window's own titlebar: a drag region (via the OS window-move
// IPC the main shell already uses) plus a close button that closes this window.
function SettingsTitlebar() {
  const bridge = getDesktopBridge();
  const controls = bridge?.windowControls;

  function handlePointerDown(event: ReactPointerEvent<HTMLElement>) {
    if ((event.target as HTMLElement).closest("button")) return;
    if (!controls) return;
    void controls.beginDrag({
      clientX: event.clientX,
      screenX: event.screenX,
      screenY: event.screenY,
      titlebarOffsetY: event.clientY,
      viewportWidth: window.innerWidth
    });
  }
  function handlePointerMove() {
    void controls?.moveDrag();
  }
  function handlePointerUp() {
    void controls?.endDrag();
  }

  return (
    <header
      className="settings-window-titlebar"
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
    >
      <span className="settings-window-title">偏好设置</span>
      <button
        aria-label="关闭"
        className="settings-window-close no-drag"
        onClick={() => void (bridge?.closeSettingsWindow?.() ?? controls?.close?.())}
        type="button"
      >
        <X aria-hidden="true" size={16} />
      </button>
    </header>
  );
}
