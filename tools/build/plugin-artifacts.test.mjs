import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import Ajv2020 from "ajv/dist/2020.js";
import { assertSelectedInventory, currentPlatform, FAKE_PLUGIN_ID, IMAGE_VIEWER_PLUGIN_ID, PPM_PLUGIN_ID, INVENTORY_FILE, MAX_UI_DOCUMENT_BYTES, parsePluginArgs, readInventory, renderTrust, sha256, stageArtifacts, validateArtifact, validateSelection } from "./plugin-artifacts.mjs";

const platform = currentPlatform();
const entry = platform === "windowsX64" ? "bin/provider.exe" : "bin/provider";

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mangoo-artifact-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}

function artifact(root, id = "com.example.storage", change = {}) {
  const dir = path.join(root, id);
  const bytes = Buffer.from(`fixture:${id}`);
  const manifest = { manifestVersion: 2, id, name: "Fixture", version: "1.2.3", engine: "process",
    entry: { [platform]: entry }, integrity: { sha256: sha256(bytes) },
    rpc: JSON.parse(fs.readFileSync(new URL("../../contracts/plugins/fixtures/valid-ui-provider-v2.json", import.meta.url), "utf8")).rpc,
    activationEvents: ["sync-enabled"], requestedPermissions: [], ...change };
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  fs.writeFileSync(path.join(dir, entry), bytes);
  fs.writeFileSync(path.join(dir, "plugin.json"), JSON.stringify(manifest));
  return dir;
}

function hostArtifact(root, change = {}) {
  const dir = path.join(root, IMAGE_VIEWER_PLUGIN_ID);
  fs.mkdirSync(dir, { recursive: true });
  const manifest = { ...JSON.parse(fs.readFileSync(new URL("../../bundled-plugins/com.mangoo.image-viewer/plugin.template.json", import.meta.url), "utf8")), ...change };
  fs.writeFileSync(path.join(dir, "plugin.json"), JSON.stringify(manifest));
  return dir;
}

function decoderFields() {
  const { rpc, activationEvents, requestedPermissions } = JSON.parse(fs.readFileSync(new URL("../../contracts/plugins/fixtures/valid-file-decoder-v2.json", import.meta.url), "utf8"));
  return { rpc, activationEvents, requestedPermissions };
}

test("default selection stages fake, viewer and decoder and removes stale generated artifacts", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const ppm = validateArtifact(artifact(root, PPM_PLUGIN_ID, decoderFields()));
  const viewer = validateArtifact(hostArtifact(root));
  const external = validateArtifact(artifact(root));
  const targetRoot = path.join(root, "target");
  stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake, viewer, ppm, external] });
  const stale = path.join(targetRoot, "debug", "bundled-plugins", external.row.id);
  assert.equal(fs.existsSync(stale), true);
  stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake, viewer, ppm] });
  assert.equal(fs.existsSync(stale), false);
  const inventory = readInventory(path.join(targetRoot, "debug", "bundled-plugins"));
  assert.deepEqual(inventory.plugins.map((row) => row.id), [FAKE_PLUGIN_ID, IMAGE_VIEWER_PLUGIN_ID, PPM_PLUGIN_ID]);
  assertSelectedInventory(inventory, []);
  assert.throws(() => assertSelectedInventory(inventory, [external]), /explicitly selected/);
  assert.equal(fs.existsSync(path.join(external.root, entry)), true);
  const backup = fs.readdirSync(path.join(targetRoot, "debug")).find((name) => name.startsWith(".bundled-plugins-backup-"));
  const previous = readInventory(path.join(targetRoot, "debug", backup, "previous"));
  assert.deepEqual(previous.plugins.map((row) => row.id).sort(), [external.row.id, FAKE_PLUGIN_ID, IMAGE_VIEWER_PLUGIN_ID, PPM_PLUGIN_ID].sort());
});

test("host-engine artifacts are exactly a schema-valid manifest pinned with the no-executable marker", (t) => {
  const root = fixture(t);
  const viewer = validateArtifact(hostArtifact(root));
  assert.deepEqual(Object.keys(viewer.row), ["id", "version", "platform", "engine", "manifestSha256"]);
  assert.equal(viewer.row.engine, "host");
  const output = path.join(root, "target", "debug", "bundled-plugins");
  stageArtifacts({ targetRoot: path.join(root, "target"), profile: "debug", artifacts: [viewer] });
  assert.deepEqual(fs.readdirSync(path.join(output, IMAGE_VIEWER_PLUGIN_ID)), ["plugin.json"]);
  const trust = renderTrust([viewer.row]);
  assert.match(trust, /"com.mangoo.image-viewer"/);
  assert.equal((trust.match(/0x00/g) ?? []).length >= 32, true);
  // Extra files, process fields and contributions are rejected.
  fs.writeFileSync(path.join(viewer.root, "extra.js"), "x");
  assert.throws(() => validateArtifact(viewer.root), /outside the current inventory/);
  fs.rmSync(path.join(viewer.root, "extra.js"));
  const withEntry = fixture(t);
  assert.throws(() => validateArtifact(hostArtifact(withEntry, { entry: { [platform]: entry } })), /must not declare entry/);
  const withPermission = fixture(t);
  assert.throws(() => validateArtifact(hostArtifact(withPermission, { requestedPermissions: [{ kind: "temp-storage", maxBytes: 1 }] })), /schema/);
  // A process row can never borrow the host marker.
  assert.throws(() => renderTrust([{ ...viewer.row, engine: undefined, sha256: "0".repeat(64) }]), /invalid plugin trust row/);
});

test("explicit artifact selection preserves exact identity, platform, executable and manifest hashes", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const ppm = validateArtifact(artifact(root, PPM_PLUGIN_ID, decoderFields()));
  const external = validateArtifact(artifact(root));
  const viewer = validateArtifact(hostArtifact(root));
  const inventory = stageArtifacts({ targetRoot: path.join(root, "target"), profile: "release", artifacts: [external, fake, viewer, ppm] });
  const loaded = readInventory(path.join(root, "target", "release", "bundled-plugins"));
  assert.deepEqual(loaded, inventory);
  assertSelectedInventory(loaded, [external]);
  const trust = renderTrust(loaded.plugins);
  assert.match(trust, /GENERATED_PLUGIN_TRUST/);
  assert.match(trust, /"com.example.storage"/);
  assert.match(trust, /"1.2.3"/);
  assert.ok(trust.includes(platform === "windowsX64" ? "Platform::WindowsX64" : "Platform::LinuxX64"));
  assert.equal((trust.match(/0x[0-9a-f]{2}/g) ?? []).length, 256);
  const changed = { ...external, row: { ...external.row, manifestSha256: "0".repeat(64) } };
  assert.throws(() => assertSelectedInventory(loaded, [changed]), /selected artifact changed/);
  assert.throws(() => assertSelectedInventory(loaded, []), /explicitly selected/);
});

test("malformed identity, version, hash and current-platform entries fail before staging", (t) => {
  const root = fixture(t);
  const invalid = [
    [{ id: "../outside" }, /plugin id/],
    [{ id: `com.${"a".repeat(255)}` }, /plugin id/],
    [{ version: '1.2.3";invalid' }, /plugin version/],
    [{ version: "1.2.3-01" }, /plugin version/],
    [{ integrity: { sha256: "0".repeat(63) } }, /plugin sha256/],
    [{ integrity: { sha256: "0".repeat(64) } }, /does not match/],
    [{ entry: {} }, /relative path/],
    ...["../provider", "bin/../../provider", "C:/provider.exe", "/provider", "bin/provider:stream", "bin/../provider", "bin/./provider", "bin/provider. ", "plugin.json", `bin/${"界".repeat(171)}`].map((value) => [{ entry: { [platform]: value } }, /relative path|metadata/]),
  ];
  for (const [change, error] of invalid) assert.throws(() => validateArtifact(artifact(root, "com.example.storage", change)), error);
});

test("duplicate IDs and incomplete external CLI arguments are rejected", (t) => {
  const root = fixture(t);
  const selected = validateArtifact(artifact(root));
  assert.throws(() => validateSelection([selected, selected]), /duplicate plugin id/);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  assert.throws(() => assertSelectedInventory({ plugins: [fake.row] }, [fake]), /duplicate plugin id/);
  assert.throws(() => parsePluginArgs(["--external-plugin"]), /incomplete/);
  assert.throws(() => parsePluginArgs(["--external-plugin", "--release"]), /incomplete/);
  assert.throws(() => parsePluginArgs(["--unexpected"]), /unknown/);
  assert.equal(parsePluginArgs(["--release-profile", "--external-plugin", root, "--external-plugin", root]).externalPlugins.length, 2);
  assert.throws(() => currentPlatform("win32", "arm64"), /unsupported/);
});

test("junction input, entry and generated output cannot escape their roots", (t) => {
  const root = fixture(t);
  const original = artifact(root);
  const linkedRoot = path.join(root, "linked");
  fs.symlinkSync(original, linkedRoot, process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => validateArtifact(linkedRoot), /symlink or junction/);
  const nested = artifact(root, "com.example.nested");
  fs.rmSync(path.join(nested, "bin"), { recursive: true });
  fs.symlinkSync(path.join(original, "bin"), path.join(nested, "bin"), process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => validateArtifact(nested), /symlink or junction/);
  const targetRoot = path.join(root, "target");
  fs.mkdirSync(path.join(targetRoot, "debug"), { recursive: true });
  fs.symlinkSync(original, path.join(targetRoot, "debug", "bundled-plugins"), process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => stageArtifacts({ targetRoot, profile: "debug", artifacts: [validateArtifact(original)] }), /symlink or junction/);
  assert.equal(fs.existsSync(path.join(original, entry)), true);
});

test("inventory rejects changed executable, changed manifest, missing and extra artifacts", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const targetRoot = path.join(root, "target");
  const output = path.join(targetRoot, "debug", "bundled-plugins");
  const reset = () => stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake] });
  reset();
  fs.writeFileSync(path.join(output, FAKE_PLUGIN_ID, entry), "modified");
  assert.throws(() => readInventory(output), /does not match/);
  reset();
  fs.appendFileSync(path.join(output, FAKE_PLUGIN_ID, "plugin.json"), " ");
  assert.throws(() => readInventory(output), /stale plugin inventory/);
  reset();
  fs.mkdirSync(path.join(output, "com.example.stale"));
  assert.throws(() => readInventory(output), /outside the current inventory/);
  reset();
  fs.rmSync(path.join(output, INVENTORY_FILE));
  assert.throws(() => readInventory(output), /ENOENT/);
});

test("inventory rejects duplicate rows and unlisted nested payloads", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const targetRoot = path.join(root, "target");
  const output = path.join(targetRoot, "debug", "bundled-plugins");
  const inventory = stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake] });
  fs.writeFileSync(path.join(output, INVENTORY_FILE), JSON.stringify({ ...inventory, plugins: [fake.row, fake.row] }));
  assert.throws(() => readInventory(output), /duplicate plugin id/);
  fs.writeFileSync(path.join(output, INVENTORY_FILE), JSON.stringify(inventory));
  fs.writeFileSync(path.join(output, FAKE_PLUGIN_ID, "bin", "unlisted"), "extra");
  assert.throws(() => readInventory(output), /outside the current inventory/);
});

test("staging rejects contained inputs and unknown existing output without changing files", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const targetRoot = path.join(root, "target");
  const output = path.join(targetRoot, "debug", "bundled-plugins");
  const inventory = stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake] });
  assert.throws(() => stageArtifacts({ targetRoot, profile: "debug", artifacts: [validateArtifact(path.join(output, FAKE_PLUGIN_ID))] }), /input must be outside/);
  assert.deepEqual(readInventory(output), inventory);
  const unknown = path.join(output, "notes.txt");
  fs.writeFileSync(unknown, "preserve");
  assert.throws(() => stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake] }), /unexpected file/);
  assert.equal(fs.readFileSync(unknown, "utf8"), "preserve");
  assert.equal(fs.readFileSync(path.join(output, INVENTORY_FILE), "utf8"), `${JSON.stringify(inventory, null, 2)}\n`);
});

test("write and swap failures preserve the last usable generated inventory", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const external = validateArtifact(artifact(root));
  const targetRoot = path.join(root, "target");
  const output = path.join(targetRoot, "debug", "bundled-plugins");
  const inventory = stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake] });
  const write = fs.writeFileSync;
  const writeMock = t.mock.method(fs, "writeFileSync", (file, ...args) => {
    if (String(file).includes(".bundled-plugins-stage-") && path.basename(file) === INVENTORY_FILE) throw new Error("simulated write failure");
    return write(file, ...args);
  });
  assert.throws(() => stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake, external] }), /simulated write failure/);
  writeMock.mock.restore();
  assert.deepEqual(readInventory(output), inventory);
  const rename = fs.renameSync;
  const renameMock = t.mock.method(fs, "renameSync", (from, to) => {
    if (String(from).includes(".bundled-plugins-stage-") && to === output) throw new Error("simulated swap failure");
    return rename(from, to);
  });
  assert.throws(() => stageArtifacts({ targetRoot, profile: "debug", artifacts: [fake, external] }), /simulated swap failure/);
  renameMock.mock.restore();
  assert.deepEqual(readInventory(output), inventory);
  assert.equal(fs.readdirSync(path.dirname(output)).some((name) => name.startsWith(".bundled-plugins-stage-")), false);
});

test("Linux manifests cannot be staged under a normalized path different from their entry", (t) => {
  const root = fixture(t);
  const source = artifact(root, "com.example.storage", { entry: { linuxX64: "bin\\provider" } });
  assert.throws(() => validateArtifact(source, "linuxX64"), /forward slashes/);
  assert.throws(() => validateArtifact(source, "unsupported"), /unsupported plugin platform/);
});

test("generated trust is accepted unchanged by rustfmt for one and multiple plugins", (t) => {
  const root = fixture(t);
  const fake = validateArtifact(artifact(root, FAKE_PLUGIN_ID));
  const external = validateArtifact(artifact(root));
  for (const rows of [[fake.row], [fake.row, external.row]]) {
    const generated = renderTrust(rows);
    const result = spawnSync("rustfmt", ["--emit", "stdout"], { input: generated, encoding: "utf8", windowsHide: true });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr);
    assert.equal(result.stdout.replaceAll("\r\n", "\n"), generated);
  }
});

function uiArtifact(root, bytes = Buffer.from("<!doctype html><script>window.fixture = true</script>"), changeUi = {}, change = {}) {
  const ui = { apiVersion: 1, entry: "ui/index.html", sha256: sha256(bytes), views: [{ id: "settings", title: "Settings", slot: "library-settings" }], ...changeUi };
  const source = artifact(root, "com.example.ui", { requestedPermissions: [{ kind: "host-ui", operations: ["core-api"] }], contributions: { configuration: [], authentication: null, presentation: null, primaryCredential: null, ui }, ...change });
  fs.mkdirSync(path.join(source, "ui"), { recursive: true });
  fs.writeFileSync(path.join(source, "ui/index.html"), bytes);
  return source;
}

test("UI inventory copies exact document and detects tamper and self-declared replacement", (t) => {
  const root = fixture(t);
  const external = validateArtifact(uiArtifact(root));
  const targetRoot = path.join(root, "target");
  const inventory = stageArtifacts({ targetRoot, profile: "debug", artifacts: [external] });
  const output = path.join(targetRoot, "debug/bundled-plugins");
  const staged = path.join(output, external.row.id);
  assert.deepEqual(readInventory(output), inventory);
  assert.deepEqual(fs.readFileSync(path.join(staged, "ui/index.html")), external.uiBytes);
  fs.writeFileSync(path.join(staged, "ui/index.html"), "changed");
  assert.throws(() => readInventory(output), /UI sha256/);
  const manifest = JSON.parse(external.manifestBytes);
  manifest.contributions.ui.sha256 = sha256(Buffer.from("changed"));
  fs.writeFileSync(path.join(staged, "plugin.json"), JSON.stringify(manifest));
  assert.throws(() => readInventory(output), /stale plugin inventory/);
  const substituted = validateArtifact(staged);
  assert.notEqual(renderTrust([external.row]), renderTrust([substituted.row]));
});

test("notification-center view survives artifact staging and inventory validation", (t) => {
  const root = fixture(t);
  const views = [{ id: "status", title: "Status", slot: "notification-center" }];
  const external = validateArtifact(uiArtifact(root, undefined, { views }));
  const targetRoot = path.join(root, "target");
  const inventory = stageArtifacts({ targetRoot, profile: "debug", artifacts: [external] });
  const output = path.join(targetRoot, "debug/bundled-plugins");
  assert.deepEqual(readInventory(output), inventory);
  const staged = JSON.parse(fs.readFileSync(path.join(output, external.row.id, "plugin.json"), "utf8"));
  assert.deepEqual(staged.contributions.ui.views, views);
});

test("UI declarations reject unknown, duplicate, oversized, ungranted and escaping input", (t) => {
  const root = fixture(t);
  for (const change of [
    { apiVersion: 2 }, { unknown: true }, { sha256: "0".repeat(64) }, { views: [] },
    { views: [{ id: "settings", title: "A", slot: "root-menu" }, { id: "settings", title: "B", slot: "library-menu" }] },
    { views: [{ id: "settings", title: "A", slot: "unknown" }] },
    ...["../ui/index.html", "/ui/index.html", "ui\\index.html", "ui/index.html:stream", "ui./index.html", "ui/./index.html"].map((entry) => ({ entry })),
  ]) assert.throws(() => validateArtifact(uiArtifact(root, undefined, change)), /plugin UI/);
  assert.throws(() => validateArtifact(uiArtifact(root, undefined, {}, { requestedPermissions: [] })), /permission/);
  assert.throws(() => validateArtifact(uiArtifact(root, undefined, {}, { requestedPermissions: [{ kind: "host-ui", operations: ["core-api", "unknown"] }] })), /permission/);
  assert.throws(() => validateArtifact(uiArtifact(root, Buffer.alloc(MAX_UI_DOCUMENT_BYTES + 1))), /bounded/);
  assert.throws(() => validateArtifact(uiArtifact(root, Buffer.from([0xff]))), /encoded data/);
  const source = uiArtifact(root);
  fs.writeFileSync(path.join(source, "extra.js"), "unlisted");
  assert.throws(() => validateArtifact(source), /outside the current inventory/);
});

test("UI directory junction is rejected before staging", (t) => {
  const root = fixture(t);
  const source = uiArtifact(root);
  const original = path.join(root, "original-ui");
  fs.renameSync(path.join(source, "ui"), original);
  fs.symlinkSync(original, path.join(source, "ui"), process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => validateArtifact(source), /symlink or junction/);
});

test("manifest JSON schema accepts UI fixture and rejects invalid UI declarations and permissions", () => {
  const schema = JSON.parse(fs.readFileSync(new URL("../../contracts/plugins/manifest.schema.json", import.meta.url), "utf8"));
  const validate = new Ajv2020({ strict: false, allErrors: true }).compile(schema);
  const valid = JSON.parse(fs.readFileSync(new URL("../../contracts/plugins/fixtures/valid-ui-provider-v2.json", import.meta.url), "utf8"));
  assert.equal(validate(valid), true, JSON.stringify(validate.errors));
  for (const edit of [
    (m) => { m.contributions.ui.apiVersion = 2; },
    (m) => { m.contributions.ui.extra = true; },
    (m) => { m.contributions.ui = null; },
    (m) => { m.contributions.ui.views = []; },
    (m) => { m.contributions.ui.views[0].slot = "unknown"; },
    (m) => { m.contributions.ui.views[0].id = "../view"; },
    (m) => { m.contributions.ui.views[0].title = "\n"; },
    (m) => { m.contributions.ui.views[0].title = "界".repeat(129); },
    (m) => { m.requestedPermissions = []; },
    (m) => { m.requestedPermissions[0].operations = ["unknown"]; },
    (m) => { m.requestedPermissions[0].operations = ["core-api", "core-api"]; },
    ...["../index.html", "ui\\index.html", "ui./index.html", "ui/./index.html", "ui/index.html:stream"].map((entry) => (m) => { m.contributions.ui.entry = entry; }),
  ]) {
    const invalid = structuredClone(valid);
    edit(invalid);
    assert.equal(validate(invalid), false, JSON.stringify(invalid.contributions.ui));
  }
  delete valid.contributions.ui;
  valid.requestedPermissions = [];
  assert.equal(validate(valid), true, JSON.stringify(validate.errors));
});
