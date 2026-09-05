import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const wizard = readFileSync(new URL("../src/features/plugins/OneDriveSetupWizard.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

describe("OneDrive setup wizard follow-up presentation", () => {
  it("uses the global overlay scrollbar only for registration overflow", () => {
    assert.match(wizard, /data-overlay-scroll-container=\{step === 1 \? "" : undefined\}/);
    assert.match(wizard, /data-step=\{step\}/);
    assert.match(styles, /\.onedrive-setup-body\[data-step="1"\]\s*\{[^}]*overflow: auto;/s);
    assert.doesNotMatch(styles, /\.onedrive-setup-body\[data-step="1"\][^{]*::-webkit-scrollbar/);
  });

  it("renders Client ID through a deliberate Megle glass field", () => {
    assert.match(wizard, /className="onedrive-setup-section onedrive-setup-centered onedrive-setup-client-id"/);
    assert.match(wizard, /<OverlayField/);
    assert.match(styles, /\.onedrive-setup-client-id \.overlay-field-control\s*\{/);
    assert.match(styles, /\.onedrive-setup-client-id \.overlay-field-control:focus-within\s*\{/);
    assert.match(styles, /\.onedrive-setup-client-id \.overlay-field-control:has\(\.overlay-field-input\[aria-invalid="true"\]\)\s*\{/);
    assert.match(styles, /\.onedrive-setup-client-id \.overlay-field\[data-disabled="true"\] \.overlay-field-control\s*\{/);
    assert.match(styles, /\.onedrive-client-id-input\s*\{[^}]*appearance: none;/s);
    assert.match(styles, /\.onedrive-client-id-input::placeholder\s*\{/);
    assert.match(styles, /\.onedrive-setup-client-id \.overlay-field-error\s*\{/);
  });

  it("centers compact Client ID and completion groups without the Windows helper copy", () => {
    assert.match(styles, /\.onedrive-setup-body:is\(\[data-step="2"\], \[data-step="4"\]\)\s*\{[^}]*align-content: center;/s);
    assert.match(wizard, /onedrive-setup-centered onedrive-setup-complete/);
    assert.match(wizard, /label="新注册应用程序"/);
    assert.doesNotMatch(wizard, /Windows 使用本机浏览器自动回调。/);
  });
});
