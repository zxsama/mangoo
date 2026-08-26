import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import type { HostPluginProjection, OneDriveConfiguration } from "@megle/core-client";
import {
  accountKindLabel,
  configurationSourceLabel,
  formatAuthRemaining,
  pluginAuthStateLabel,
  pluginAuthPrimaryAction,
} from "../src/features/plugins/pluginAuthModel.ts";

const configuration: OneDriveConfiguration = {
  source: "bundled",
  configured: true,
  readOnly: false,
  revision: 1,
  errorCode: null,
};

function plugin(overrides: Partial<HostPluginProjection> = {}): HostPluginProjection {
  return {
    pluginId: "com.megle.onedrive",
    selectedVersion: "2.0.0",
    knownGoodVersion: "2.0.0",
    enabledIntent: true,
    runtimeState: "ready",
    authState: "authorization-required",
    capabilities: ["sync-provider.v1"],
    requestedPermissions: [],
    grantedPermissions: [],
    permissionRequired: false,
    accountUuid: null,
    accountDisplayName: null,
    accountKind: null,
    compatibilityReason: null,
    backoffUntil: null,
    lastHealthyAt: null,
    lastErrorCode: null,
    lastErrorDetail: null,
    ...overrides,
  };
}

describe("OneDrive plugin auth presentation", () => {
  it("derives one primary action from the Host projection", () => {
    assert.equal(pluginAuthPrimaryAction(null, configuration), "unavailable");
    assert.equal(pluginAuthPrimaryAction(plugin({ enabledIntent: false }), configuration), "enable");
    assert.equal(pluginAuthPrimaryAction(plugin({ permissionRequired: true }), configuration), "approve");
    assert.equal(pluginAuthPrimaryAction(plugin(), { ...configuration, configured: false }), "configure");
    assert.equal(pluginAuthPrimaryAction(plugin({ authState: "authorizing" }), configuration), "wait");
    assert.equal(pluginAuthPrimaryAction(plugin({ authState: "authenticated" }), configuration), "manage");
    assert.equal(pluginAuthPrimaryAction(plugin({ authState: "expired" }), configuration), "retry");
    assert.equal(pluginAuthPrimaryAction(plugin(), configuration), "connect");
  });

  it("shows account kind and configuration source without exposing IDs", () => {
    assert.equal(accountKindLabel("personal"), "OneDrive Personal");
    assert.equal(accountKindLabel("business"), "OneDrive Business");
    assert.equal(configurationSourceLabel("environment"), "部署环境（只读）");
    assert.equal(configurationSourceLabel("bundled"), "Megle 内置应用配置");
    assert.equal(formatAuthRemaining(61_000, 0), "1:01");
    assert.equal(pluginAuthStateLabel("signed-out"), "已退出");
    assert.equal(pluginAuthStateLabel("authorization-required"), "需要授权");
    assert.equal(pluginAuthStateLabel("expired"), "授权已过期");
    assert.equal(pluginAuthStateLabel("error"), "授权失败");
  });

  it("keeps response URI transient and routes actions through the shared controller", () => {
    const controller = readFileSync(new URL("../src/features/plugins/usePluginAuth.ts", import.meta.url), "utf8");
    const setup = readFileSync(new URL("../src/features/plugins/OneDriveAccountSetup.tsx", import.meta.url), "utf8");
    const wizard = readFileSync(new URL("../src/features/library/ManagedLibraryWizard.tsx", import.meta.url), "utf8");
    assert.match(controller, /useQuery\(/);
    assert.match(controller, /cancelPluginAuth/);
    assert.match(controller, /setResponseUri\(""\)/);
    assert.doesNotMatch(controller, /localStorage|sessionStorage/);
    assert.match(setup, /manual-response-uri/);
    assert.match(setup, /完整 response URI/);
    assert.match(setup, /MEGLE_ONEDRIVE_CLIENT_ID/);
    assert.match(setup, /不需要 client secret/);
    assert.match(setup, /openAuthorizationUrl/);
    assert.match(setup, /确认切换 OneDrive 账号/);
    assert.match(wizard, /authResumeRef/);
    assert.match(wizard, /active\.contextRevision !== intent\.contextRevision/);
  });

  it("keeps plugin maintenance details out of the primary workflow", () => {
    const setup = readFileSync(new URL("../src/features/plugins/OneDriveAccountSetup.tsx", import.meta.url), "utf8");
    const card = readFileSync(new URL("../src/features/plugins/PluginCard.tsx", import.meta.url), "utf8");
    const detail = readFileSync(new URL("../src/features/plugins/PluginDetail.tsx", import.meta.url), "utf8");

    assert.match(setup, /<details[\s\S]*onedrive-advanced-configuration/);
    assert.match(setup, /<summary[\s\S]*高级配置/);
    assert.match(setup, /检查更改/);
    assert.doesNotMatch(setup, /预览更改|收起高级配置/);
    assert.match(card, /aria-label=\{toggleLabel\}/);
    assert.doesNotMatch(card, /plugin-card-error|plugin-toggle-label/);
    assert.match(detail, /<details className="plugin-disclosure plugin-technical-details">/);
    assert.match(detail, /技术信息/);
    assert.match(detail, /tone="danger"/);
    assert.doesNotMatch(detail, /<dt>Intent<\/dt>|<dt>Authentication<\/dt>|plugin-detail-id/);
  });
});
