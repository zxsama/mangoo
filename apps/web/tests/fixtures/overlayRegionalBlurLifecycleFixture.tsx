import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { LiquidGlassLayer } from "../../src/design/liquid-glass";
import {
  AnchoredOverlay,
  ModalSurface,
  OverlayProvider,
  pointOverlayAnchor
} from "../../src/design/overlays";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";

type FixturePhase = "closed" | "root" | "nested" | "modal";

const rootAnchor = pointOverlayAnchor(160, 96);
const nestedAnchor = pointOverlayAnchor(390, 136);

function RegionalBlurLifecycleFixture() {
  const [phase, setPhase] = useState<FixturePhase>("closed");
  const rootOpen = phase !== "closed";
  const nestedOpen = phase === "nested" || phase === "modal";
  const modalOpen = phase === "modal";

  useEffect(() => {
    const handlePhase = (event: Event) => {
      setPhase((event as CustomEvent<FixturePhase>).detail);
    };
    window.addEventListener("mangoo-regional-blur-phase", handlePhase);
    return () => window.removeEventListener("mangoo-regional-blur-phase", handlePhase);
  }, []);

  return (
    <LiquidGlassLayer>
      <main style={{ minHeight: "100vh", padding: 24 }}>
        <h1>Regional SourceGraphic fixture</h1>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(5, 1fr)", gap: 8 }}>
          {Array.from({ length: 15 }, (_, index) => (
            <article key={index} style={{ minHeight: 92, background: index % 2 ? "#345" : "#712" }}>
              independent paint chunk {index}
            </article>
          ))}
        </div>
      </main>

      <AnchoredOverlay
        anchor={rootAnchor}
        ariaLabel="Regional root"
        className="regional-lifecycle-root"
        id="regional-lifecycle-root"
        onDismiss={() => undefined}
        open={rootOpen}
        preferredWidth={280}
        rootId="library"
      >
        <div style={{ height: 190, padding: 12 }}>Root popup content</div>
      </AnchoredOverlay>

      <AnchoredOverlay
        anchor={nestedAnchor}
        ariaLabel="Regional nested"
        className="regional-lifecycle-nested"
        id="regional-lifecycle-nested"
        onDismiss={() => undefined}
        open={nestedOpen}
        parentId="regional-lifecycle-root"
        placement="right-start"
        preferredWidth={200}
        rootId="library"
        surfaceRole="nested"
      >
        <div style={{ height: 130, padding: 12 }}>Nested popup content</div>
      </AnchoredOverlay>

      <ModalSurface
        ariaLabelledBy="regional-lifecycle-modal-title"
        id="regional-lifecycle-modal"
        onDismiss={() => undefined}
        open={modalOpen}
        rootId="library"
      >
        <h2 id="regional-lifecycle-modal-title">Modal content</h2>
      </ModalSurface>
    </LiquidGlassLayer>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("regional blur lifecycle root is missing");
root.style.filter = "brightness(1)";
createRoot(root).render(
  <StrictMode>
    <OverlayProvider>
      <RegionalBlurLifecycleFixture />
    </OverlayProvider>
  </StrictMode>
);
