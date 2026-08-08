import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { useSecondaryWindowInterfaceStyle } from "../../src/app-shell/useSecondaryWindowInterfaceStyle";
import { LiquidGlassLayer, LiquidGlassSurface } from "../../src/design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayProvider,
  OverlaySurface
} from "../../src/design/overlays";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";
import "./overlayAppearanceVisualFixture.css";

function OverlayAppearanceVisualFixture() {
  useSecondaryWindowInterfaceStyle();
  const [modalOpen, setModalOpen] = useState(true);

  useEffect(() => {
    const handleModalToggle = (event: Event) => {
      setModalOpen((event as CustomEvent<boolean>).detail !== false);
    };
    window.addEventListener("megle-overlay-fixture-modal", handleModalToggle);
    return () => window.removeEventListener("megle-overlay-fixture-modal", handleModalToggle);
  }, []);

  return (
    <LiquidGlassLayer>
      <main className="overlay-appearance-fixture" data-overlay-appearance-fixture="ready">
        <LiquidGlassSurface
          as="aside"
          className="library-sidebar overlay-appearance-library-zone"
          data-appearance-surface="library-navigation"
          tone="chrome"
        >
          Library navigation material
        </LiquidGlassSurface>

        <section
          className="notification-window-body overlay-appearance-notification-window"
          data-appearance-surface="notification-body"
        >
          <nav className="notification-window-nav" data-appearance-surface="notification-nav">
            <div className="notification-window-nav-heading">通知中心</div>
            <button className="notification-window-tab active" type="button">当前状态</button>
          </nav>
          <main className="notification-window-content" data-appearance-surface="notification-content">
            <LiquidGlassSurface
              as="section"
              className="toolbar overlay-appearance-center-surface"
              data-appearance-surface="center-surface"
              tone="chrome"
            >
              Center material
            </LiquidGlassSurface>
            <div className="notification-history-filters overlay-appearance-center-probes">
              <label className="overlay-field">
                <span className="overlay-field-label">Search</span>
                <span className="overlay-field-control" data-appearance-surface="field-control">
                  <input className="overlay-field-input" defaultValue="fixture" />
                </span>
              </label>
              <label className="overlay-field">
                <span className="overlay-field-label">Status</span>
                <span className="overlay-field-control" data-appearance-surface="select-control">
                  <select className="overlay-field-select" data-appearance-surface="select-element" defaultValue="all">
                    <option value="all">All</option>
                  </select>
                </span>
              </label>
              <div className="move-tree overlay-appearance-readable-inner" data-appearance-surface="readable-inner">
                Readable inner region
              </div>
              <div className="overlay-titlebar-menu">
                <button
                  className="overlay-menu-item"
                  data-appearance-surface="selected-fill"
                  data-selected="true"
                  type="button"
                >
                  Selected fill
                </button>
              </div>
            </div>
          </main>
        </section>

        <div className="overlay-appearance-dialog-probes">
          <OverlaySurface
            className="overlay-appearance-dialog-surface"
            data-appearance-surface="popover"
            surfaceRole="popover"
          >
            <button
              data-fallback-probe="popover"
              onClick={(event) => { event.currentTarget.dataset.fallbackActivated = "true"; }}
              type="button"
            >
              Popover material
            </button>
          </OverlaySurface>
          <OverlaySurface
            className="overlay-appearance-dialog-surface"
            data-appearance-surface="nested"
            surfaceRole="nested"
          >
            <button
              data-fallback-probe="nested"
              onClick={(event) => { event.currentTarget.dataset.fallbackActivated = "true"; }}
              type="button"
            >
              Nested material
            </button>
          </OverlaySurface>
          <OverlaySurface
            className="context-menu overlay-appearance-dialog-surface"
            data-appearance-surface="context"
            surfaceRole="popover"
          >
            <button
              data-fallback-probe="context"
              onClick={(event) => { event.currentTarget.dataset.fallbackActivated = "true"; }}
              type="button"
            >
              Context material
            </button>
          </OverlaySurface>
        </div>

        <div className="grid-surface overlay-appearance-unaffected" data-appearance-surface="media-grid">
          Media grid invariant
        </div>
        <div className="preview-canvas overlay-appearance-unaffected" data-appearance-surface="preview-canvas">
          Preview canvas invariant
        </div>
      </main>

      <ModalSurface
        ariaLabelledBy="appearance-modal-heading"
        className="file-operation-dialog overlay-appearance-modal"
        id="appearance-matrix-modal"
        onDismiss={() => undefined}
        open={modalOpen}
        rootId="file-operation"
      >
        <OverlayDialogHeader title="Modal material" titleId="appearance-modal-heading" />
        <OverlayDialogBody>
          <button
            data-fallback-probe="modal"
            onClick={(event) => { event.currentTarget.dataset.fallbackActivated = "true"; }}
            type="button"
          >
            Controlled production modal surface.
          </button>
        </OverlayDialogBody>
      </ModalSurface>
    </LiquidGlassLayer>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("overlay appearance fixture root is missing");
createRoot(root).render(
  <StrictMode>
    <OverlayProvider>
      <OverlayAppearanceVisualFixture />
    </OverlayProvider>
  </StrictMode>
);
