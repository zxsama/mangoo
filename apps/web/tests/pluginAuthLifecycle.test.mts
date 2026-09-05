import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const runner = fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url));
let server: ViteDevServer;
let result: any;

before(async () => {
  server = await createServer({ root: webRoot, logLevel: "error", server: { host: "127.0.0.1", port: 0, strictPort: false } });
  await server.listen();
  const address = server.httpServer?.address(); assert.ok(address && typeof address === "object");
  const electronModule = await import("electron"); const electronPath = electronModule.default as unknown as string;
  const environment = { ...process.env }; delete environment.ELECTRON_RUN_AS_NODE;
  result = await new Promise((resolve, reject) => {
    const child = spawn(electronPath, [runner, `http://127.0.0.1:${address.port}/tests/fixtures/plugin-auth-lifecycle.html`], { cwd: webRoot, env: environment, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = ""; child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8"); child.stdout.on("data", (chunk) => stdout += chunk); child.stderr.on("data", (chunk) => stderr += chunk);
    child.on("exit", () => { const marker = "OVERLAY_LIFECYCLE_RESULT:"; const index = stdout.lastIndexOf(marker); index < 0 ? reject(new Error(`${stdout}\n${stderr}`)) : resolve(JSON.parse(stdout.slice(index + marker.length).trim())); }); child.on("error", reject);
  });
  assert.equal(result.ok, true, result.error);
});
after(async () => server?.close());

describe("mounted OneDrive auth lifecycle", () => {
  it("keeps the configuration action stable and exposes account management", () => assert.deepEqual(result.authenticatedStableAction, { stableLabel: true, accountActionsVisible: true, singleConnectedStatus: true, accountNameVisible: true, effectiveStatusHidden: true, opensConfiguration: true }));
  it("keeps a failed cancel retryable", () => assert.deepEqual(result.cancelFailure, { flowRetained: true, retryAvailable: true }));
  it("dispatches a manual URI and clears terminal and unmounted state", () => { assert.deepEqual(result.manualCompletion, { dispatched: true, terminalCleared: true }); assert.deepEqual(result.unmountCleanup, { responseUriGone: true, flowGone: true }); });
  it("keeps enablement explicit while preparing only requested grants for configuration", () => { assert.deepEqual(result.disabledConfiguration, { wizardOpened: true, enableStayedExplicit: true }); assert.deepEqual(result.configurationPreparation, { grantedRequestedPermissions: true, separateApprovalHidden: true }); });
  it("renders configuration and connection state in one status location", () => assert.deepEqual(result.stableActionStatus, { ineffectiveStatusVisible: true, connectedStatusAbsent: true, duplicateStatusAbsent: true, effectiveStatusHidden: true }));
  it("keeps the stable action while retrying a degraded Provider", () => assert.deepEqual(result.retryState, { stableLabel: true, runtimeRetried: true }));
  it("enforces sequential setup and single-click configuration", () => {
    assert.deepEqual(result.wizardGates, { combinedSetupBlocked: true, nextEnabledForValidation: true, requiredFieldError: true, formatFieldError: true, clientIdAccepted: true });
    assert.deepEqual(result.reopenState, { stalePlanGone: true, forwardLabelStable: true });
    assert.deepEqual(result.planFailure, { fieldLocal: true, stayedOnStep: true });
    assert.deepEqual(result.commitFailure, { fieldLocal: true, inputPreserved: true });
    assert.deepEqual(result.configConfirm, { confirmed: true });
    assert.deepEqual(result.backNavigation, { returnedToClientId: true, didNotSkipToLogin: true });
    assert.equal(result.configNoop.committed, true);
    assert.deepEqual(result.readOnly, { editorHidden: true, managedStatusVisible: true });
  });
  it("keeps the wizard shell stable, scroll-safe, and on shared action buttons", () => assert.deepEqual(result.wizardLayout, {
    stableHeight: true,
    registrationScroll: { markedForOverlay: true, overflowY: "auto" },
    clientIdPresentation: { centered: true, nativeAppearanceRemoved: true, deliberateSurface: true, placeholderPresent: true },
    footerVisible: true,
    unifiedActions: true,
  }));
  it("surfaces a blocked Entra registration-page launch", () => assert.equal(result.portalOpenFailureVisible, true));
  it("opts plugin navigation out of proximity and local pointer glow", () => { assert.deepEqual(result.pointerOptOut, { declared: true, outsideInactive: true, insideInactive: true }); });
  it("keeps plugin selection and the relocated detail switch working", () => { assert.deepEqual(result.cardInteraction, { selected: true, toggled: true }); });
  it("renders concise plugin rows and an accessible technical disclosure", () => { assert.deepEqual(result.conciseCard, { friendlyNameVisible: true, rawIdHidden: true, oneLineOrder: true, versionAdjacent: true, rawErrorHidden: true, toggleMovedToDetail: true, toggleAccessible: true }); assert.deepEqual(result.tabRelationship, { controlsPanel: true, panelLabelledByTab: true, panelRole: true }); assert.deepEqual(result.technicalDisclosure, { collapsedInitially: true, pointerGlowDisabled: true, chevronBeforeLabel: true, diagnosticsNested: true, removalNested: true }); assert.equal(result.technicalOpenedWithKeyboard, true); });
  it("keeps nested OneDrive setup in the managed-library overlay family and restores the flow", () => { assert.deepEqual(result.wizardResume, { focusRestored: true, nestedWizardPersisted: true, remoteReloaded: true }); assert.deepEqual(result.wizardRevision, { staleIntentCleared: true }); });
});
