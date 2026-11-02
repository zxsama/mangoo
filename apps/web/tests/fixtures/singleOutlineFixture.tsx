import { createRoot } from "react-dom/client";
import { useLayoutEffect } from "react";
import { LibraryIconPicker } from "../../src/features/library/libraryGlyph";
import { LiquidGlassButton, LiquidGlassLayer, LiquidGlassSurface } from "../../src/design/liquid-glass";
import { OverlayField, OverlayProvider, OverlaySelect } from "../../src/design/overlays";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

function AppearanceProbe() {
  useLayoutEffect(() => {
    document.querySelector<HTMLElement>('.library-icon-picker-swatch[aria-checked="true"]')!.id = "swatch";
  }, []);
  return <div className="library-appearance-editor" style={{ width: 340 }}>
    <LibraryIconPicker icon="camera" color="#5ce1a6" onPick={() => undefined} />
  </div>;
}

createRoot(document.getElementById("root")!).render(
  <LiquidGlassLayer><OverlayProvider><main style={{ padding: 32, display: "grid", gap: 20, width: 620 }}>
    <LiquidGlassSurface className="overlay-surface" style={{ padding: 20, display: "flex", gap: 12 }}>
      <LiquidGlassButton id="glass">Change folder</LiquidGlassButton>
      <LiquidGlassButton id="primary" tone="primary" active>Create</LiquidGlassButton>
      <LiquidGlassButton id="disabled" disabled>Disabled</LiquidGlassButton>
    </LiquidGlassSurface>
    <div style={{ display: "flex", gap: 12 }}>
      <button id="ordinary" className="settings-action">Settings action</button>
      <button id="default">Default</button>
      <div className="overlay-titlebar-menu"><button id="borderless" className="overlay-menu-item" data-overlay-menu-item="true">Menu item</button></div>
      <button id="tab" className="notification-window-tab">Tab</button>
    </div>
    <div className="managed-library-wizard"><OverlayField id="field" label="Name" defaultValue="Library" /></div>
    <div className="plugin-setup-configuration"><OverlayField id="plugin-field" className="plugin-configuration-input" label="Plugin field" error="Required" /></div>
    <OverlaySelect id="select" label="Layout" defaultValue="grid"><option value="grid">Grid</option><option value="list">List</option></OverlaySelect>
    <div style={{ display: "flex", gap: 16 }}>
      <label className="plugin-toggle"><input id="toggle" type="checkbox" defaultChecked /><span className="plugin-toggle-track"><span className="plugin-toggle-thumb" /></span></label>
      <AppearanceProbe />
      <div className="settings-style-slider-control"><input id="slider" type="range" defaultValue={50} aria-label="Opacity" /></div>
    </div>
  </main></OverlayProvider></LiquidGlassLayer>
);
