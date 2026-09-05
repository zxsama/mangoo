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
import {
  deriveOneDriveSetupStep,
  isValidOneDriveClientId,
  oneDriveSetupRecoveryStep,
} from "../src/features/plugins/oneDriveSetupModel.ts";

const configuration: OneDriveConfiguration = {
  source: "host-override",
  configured: true,
  readOnly: false,
  revision: 1,
  errorCode: null,
};

function plugin(overrides: Partial<HostPluginProjection> = {}): HostPluginProjection {
  return {
    pluginId: "com.megle.onedrive",
    name: "OneDrive Sync",
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
    assert.equal(pluginAuthPrimaryAction(plugin({ enabledIntent: false }), configuration), "configure");
    assert.equal(pluginAuthPrimaryAction(plugin({ permissionRequired: true }), configuration), "configure");
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
    assert.equal(configurationSourceLabel("host-override"), "此设备高级覆盖");
    assert.equal(formatAuthRemaining(61_000, 0), "1:01");
    assert.equal(pluginAuthStateLabel("signed-out"), "已退出");
    assert.equal(pluginAuthStateLabel("authorization-required"), "需要授权");
    assert.equal(pluginAuthStateLabel("expired"), "授权已过期");
    assert.equal(pluginAuthStateLabel("error"), "授权失败");
  });

  it("enforces wizard gates and derives safe recovery steps", () => {
    assert.equal(isValidOneDriveClientId("11111111-1111-4111-8111-111111111111"), true);
    assert.equal(isValidOneDriveClientId("00000000-0000-0000-0000-000000000000"), false);
    assert.equal(isValidOneDriveClientId("directory-id"), false);
    assert.equal(deriveOneDriveSetupStep({ ...configuration, configured: false }, false), 1);
    assert.equal(deriveOneDriveSetupStep(configuration, false), 3);
    assert.equal(deriveOneDriveSetupStep(configuration, true), 4);
    assert.equal(oneDriveSetupRecoveryStep("AADSTS700016 application not found"), 2);
    assert.equal(oneDriveSetupRecoveryStep("redirect URI mismatch"), 1);
    assert.equal(oneDriveSetupRecoveryStep("network unavailable"), null);
  });

  it("keeps response URI transient and routes actions through the shared controller", () => {
    const controller = readFileSync(new URL("../src/features/plugins/usePluginAuth.ts", import.meta.url), "utf8");
    const setup = readFileSync(new URL("../src/features/plugins/OneDriveAccountSetup.tsx", import.meta.url), "utf8");
    const wizard = readFileSync(new URL("../src/features/library/ManagedLibraryWizard.tsx", import.meta.url), "utf8");
    assert.match(controller, /useQuery\(/);
    assert.match(controller, /cancelPluginAuth/);
    assert.match(controller, /requestOAuthReturnFocus/);
    assert.match(controller, /requestOAuthReturnFocus\(\)\.catch\(\(\) => undefined\)/);
    assert.match(controller, /locallyStartedFlowRef/);
    assert.match(controller, /prepareConfiguration/);
    assert.match(controller, /permissions: plugin\.requestedPermissions/);
    assert.match(controller, /setResponseUri\(""\)/);
    assert.doesNotMatch(controller, /localStorage|sessionStorage/);
    const setupWizard = readFileSync(new URL("../src/features/plugins/OneDriveSetupWizard.tsx", import.meta.url), "utf8");
    assert.match(setupWizard, /manual-response-uri/);
    assert.match(setupWizard, /登录返回地址/);
    assert.doesNotMatch(setupWizard, /Client Secret/);
    assert.match(setupWizard, /openAuthorizationUrl/);
    assert.doesNotMatch(setupWizard, /password|access token|refresh token/i);
    assert.match(setupWizard, /配置进度/);
    assert.match(setupWizard, /http:\/\/localhost\/plugin-auth\/callback/);
    assert.match(setupWizard, /移动和桌面应用程序/);
    assert.match(setupWizard, /Authentication \(Preview\) → 设置 → 允许公共客户端流 → 已启用/);
    assert.doesNotMatch(setupWizard, /User\.Read/);
    assert.doesNotMatch(setupWizard, /<button\b/);
    assert.match(setupWizard, /onedrive-setup-actions/);
    assert.match(setupWizard, /allConfigurationChecksComplete/);
    assert.match(setupWizard, /data-overlay-scroll-container/);
    assert.match(setupWizard, /应用程序\(客户端\) ID/);
    assert.match(setupWizard, /clientIdValid/);
    assert.match(setupWizard, /OverlayField/);
    assert.match(setupWizard, /应用注册 → 新注册 → 新注册应用程序/);
    assert.match(setupWizard, /Authentication \(Preview\) → 重定向 URI 配置 → 添加重定向 URI → 移动和桌面应用程序 → 重定向 URI/);
    assert.match(setupWizard, /API 权限 → 添加权限 → Microsoft Graph → 委托的权限 → Files\.ReadWrite\.AppFolder → 添加权限/);
    assert.match(setupWizard, /await auth\.saveConfiguration\(normalizedClientId\)/);
    assert.doesNotMatch(setupWizard, /检查配置|确认并继续|配置将发生变化|配置有效|修改 Client ID|onedrive-client-id-card|粘贴后/);
    assert.match(setupWizard, /auth\.plugin\?\.authState === "authenticated"/);
    assert.match(setup, /重新配置/);
    assert.match(setup, /切换账号/);
    assert.match(setup, /退出账号/);
    assert.match(setup, /onedrive-connection-status/);
    assert.doesNotMatch(setup, /配置状态|configurationStatus/);
    assert.doesNotMatch(setup, /accountStatusDescription/);
    assert.doesNotMatch(setup, />登录 Microsoft 账号<|>继续登录<|>重新连接</);
    assert.match(setup, /authenticated \? 2 : deriveOneDriveSetupStep\(auth\.configuration, false\)/);
    assert.match(setup, /auth\.plugin\?\.runtimeState === "degraded" \|\| auth\.plugin\?\.runtimeState === "crashed-backoff"/);
    assert.doesNotMatch(setup, /<button\b/);
    assert.match(wizard, /authResumeRef/);
    assert.match(wizard, /active\.contextRevision !== intent\.contextRevision/);
  });

  it("keeps plugin maintenance details out of the primary workflow", () => {
    const setup = readFileSync(new URL("../src/features/plugins/OneDriveAccountSetup.tsx", import.meta.url), "utf8");
    const card = readFileSync(new URL("../src/features/plugins/PluginCard.tsx", import.meta.url), "utf8");
    const detail = readFileSync(new URL("../src/features/plugins/PluginDetail.tsx", import.meta.url), "utf8");

    assert.match(setup, /OneDriveSetupWizard/);
    assert.match(setup, /配置 OneDrive/);
    assert.match(setup, /重新配置/);
    assert.match(setup, /切换账号/);
    assert.match(setup, /退出账号/);
    assert.doesNotMatch(setup, /高级配置|检查更改|预览更改|收起高级配置/);
    assert.doesNotMatch(setup, /重新扫描/);
    assert.doesNotMatch(card, /plugin-toggle|onToggleEnabled|aria-label=\{toggleLabel\}/);
    assert.match(card, /data-pointer-glow="off"/);
    assert.match(card, /role="tab"/);
    assert.match(card, /aria-selected=\{selected\}/);
    assert.match(card, /aria-controls=\{selected \? pluginPanelId\(plugin\.pluginId\) : undefined\}/);
    assert.match(card, /id=\{pluginTabId\(plugin\.pluginId\)\}/);
    assert.match(card, /onKeyDown=\{handleTabKeyDown\}/);
    assert.match(card, /\{plugin\.name\}/);
    assert.match(card, /plugin-card-identity/);
    assert.match(card, /plugin-card-icon/);
    assert.doesNotMatch(card, /LiquidGlassSurface|aria-pressed/);
    assert.doesNotMatch(card, /plugin-card-error|plugin-toggle-label/);
    assert.match(detail, /<details className="plugin-disclosure plugin-technical-details">/);
    assert.match(detail, /<summary[^>]*data-pointer-glow="off"[\s\S]*plugin-disclosure-chevron[\s\S]*plugin-disclosure-label">技术信息/);
    assert.match(detail, /plugin-detail-title">\{plugin\.name\}/);
    assert.match(detail, /plugin-detail-enable-row/);
    assert.match(detail, /aria-label=\{toggleLabel\}/);
    assert.match(detail, /aria-labelledby=\{pluginTabId\(plugin\.pluginId\)\}/);
    assert.match(detail, /id=\{pluginPanelId\(plugin\.pluginId\)\}/);
    assert.match(detail, /role="tabpanel"/);
    assert.match(detail, /tone="danger"/);
    assert.doesNotMatch(detail, /账号或资源库仍引用此 Provider 时无法移除/);
    assert.doesNotMatch(detail, /<dt>Intent<\/dt>|<dt>Authentication<\/dt>|plugin-detail-id/);
  });
});
