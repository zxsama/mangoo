import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const wizard = readFileSync(new URL("../src/features/plugins/PluginSetupWizard.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

describe("Plugin setup wizard presentation", () => {
  it("uses the global overlay scrollbar only for registration overflow", () => {
    assert.match(wizard, /data-overlay-scroll-container=""/);
    assert.match(wizard, /data-step=\{step\}/);
    assert.match(styles, /\.plugin-setup-body:is\(\[data-step="1"\], \[data-step="2"\], \[data-step="3"\]\)\s*\{[^}]*overflow: auto;/s);
    assert.doesNotMatch(styles, /\.plugin-setup-body\[data-step="1"\][^{]*::-webkit-scrollbar/);
  });

  it("renders configuration through the shared Mangoo glass field", () => {
    assert.match(wizard, /className="plugin-setup-section plugin-setup-configuration"/);
    assert.match(wizard, /<OverlayField/);
    assert.match(styles, /\.plugin-setup-configuration \.overlay-field-control\s*\{/);
    assert.match(styles, /\.plugin-setup-configuration \.overlay-field-control:focus-within\s*\{/);
    assert.match(styles, /\.plugin-setup-configuration \.overlay-field-control:has\(\.overlay-field-input\[aria-invalid="true"\]\)\s*\{/);
    assert.match(styles, /\.plugin-setup-configuration \.overlay-field\[data-disabled="true"\] \.overlay-field-control\s*\{/);
    assert.match(styles, /\.plugin-configuration-input\s*\{[^}]*appearance: none;/s);
    assert.match(styles, /\.plugin-configuration-input::placeholder\s*\{/);
    assert.match(styles, /\.plugin-setup-configuration \.overlay-field-error\s*\{/);
  });

  it("centers the completion group and renders declared instructions", () => {
    assert.match(styles, /\.plugin-setup-body\[data-step="4"\]\s*\{[^}]*align-content: center;/s);
    assert.match(wizard, /plugin-setup-centered plugin-setup-complete/);
    assert.match(wizard, /instruction.title/);
    assert.doesNotMatch(wizard, /Windows 使用本机浏览器自动回调。/);
  });
});
