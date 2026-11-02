import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";

export const INVENTORY_FILE = ".inventory.json";
export const FAKE_PLUGIN_ID = "com.mangoo.fake-sync";
export const PPM_PLUGIN_ID = "com.mangoo.ppm-decoder";
const ID = /^[a-z0-9]+(\.[a-z0-9][a-z0-9-]*)+$/;
const VERSION = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const SHA256 = /^[0-9a-f]{64}$/;
export const MAX_UI_DOCUMENT_BYTES = 8 * 1024 * 1024;
const UI_ENTRY = /^(?:[A-Za-z0-9_-](?:[A-Za-z0-9._-]*[A-Za-z0-9_-])?\/)*[A-Za-z0-9_-][A-Za-z0-9._-]*\.html$/;
const UI_SLOTS = ["library-menu", "library-settings", "plugin-detail", "root-menu", "notification-center"];
const UI_OPERATIONS = ["core-api", "pick-folder", "reveal-path", "clipboard-write", "external-link"];
const validateManifestSchema = new Ajv2020({ strict: false, allErrors: true }).compile(
  JSON.parse(fs.readFileSync(new URL("../../contracts/plugins/manifest.schema.json", import.meta.url), "utf8"))
);

function exactKeys(value, keys) {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

function readBounded(file, limit, label) {
  assertNoLinks(file);
  const fd = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.size > limit) throw new Error(`${label} must be a bounded file`);
    const bytes = Buffer.alloc(stat.size + 1);
    let length = 0;
    while (length < bytes.length) {
      const count = fs.readSync(fd, bytes, length, bytes.length - length, null);
      if (!count) break;
      length += count;
    }
    if (length !== stat.size) throw new Error(`${label} changed while reading`);
    return bytes.subarray(0, length);
  } finally { fs.closeSync(fd); }
}

function readUi(root, manifest, executableEntry) {
  const permissions = manifest.requestedPermissions ?? [];
  if (!Array.isArray(permissions)) throw new Error("invalid plugin permissions");
  for (const permission of permissions.filter((p) => p?.kind === "host-ui")) {
    if (!exactKeys(permission, ["kind", "operations"]) || !Array.isArray(permission.operations)
        || !permission.operations.length || permission.operations.length > 5
        || new Set(permission.operations).size !== permission.operations.length
        || permission.operations.some((op) => !UI_OPERATIONS.includes(op))) throw new Error("invalid host-ui permission");
  }
  if (!manifest.contributions || !Object.hasOwn(manifest.contributions, "ui")) return {};
  const ui = manifest.contributions.ui;
  if (!exactKeys(ui, ["apiVersion", "entry", "sha256", "views"]) || ui.apiVersion !== 1
      || typeof ui.entry !== "string" || ui.entry.length > 512 || !UI_ENTRY.test(ui.entry)
      || !SHA256.test(ui.sha256 ?? "") || !Array.isArray(ui.views) || !ui.views.length || ui.views.length > 16) throw new Error("invalid plugin UI contribution");
  const ids = new Set();
  for (const view of ui.views) {
    if (!exactKeys(view, ["id", "title", "slot"]) || typeof view.id !== "string" || !/^[A-Za-z][A-Za-z0-9-]{0,63}$/.test(view.id)
        || ids.has(view.id) || typeof view.title !== "string" || !view.title.trim() || [...view.title].length > 128
        || /[\x00-\x1f\x7f-\x9f]/.test(view.title) || !UI_SLOTS.includes(view.slot)) throw new Error("invalid plugin UI view");
    ids.add(view.id);
  }
  if (!permissions.some((p) => p?.kind === "host-ui" && p.operations.includes("core-api"))) throw new Error("plugin UI requires host-ui core-api permission");
  const entry = entryPath(root, ui.entry);
  if (entry.relative.toLowerCase() === executableEntry.toLowerCase()) throw new Error("plugin UI entry collides with executable");
  const uiBytes = readBounded(entry.absolute, MAX_UI_DOCUMENT_BYTES, "plugin UI document");
  new TextDecoder("utf-8", { fatal: true }).decode(uiBytes);
  if (sha256(uiBytes) !== ui.sha256) throw new Error("plugin UI sha256 does not match manifest");
  return { uiBytes, ui: { entry: entry.relative, sha256: ui.sha256 } };
}

export function currentPlatform(platform = process.platform, arch = process.arch) {
  if (arch !== "x64" || !["win32", "linux"].includes(platform)) throw new Error(`unsupported plugin platform: ${platform}/${arch}`);
  return platform === "win32" ? "windowsX64" : "linuxX64";
}

export function parsePluginArgs(argv) {
  const options = { release: false, releaseProfile: false, externalPlugins: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--release") options.release = options.releaseProfile = true;
    else if (argument === "--release-profile") options.releaseProfile = true;
    else if (argument === "--external-plugin" && argv[index + 1] && !argv[index + 1].startsWith("--")) options.externalPlugins.push(path.resolve(argv[++index]));
    else throw new Error(`unknown or incomplete argument: ${argument}`);
  }
  return options;
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

// Every existing ancestor is checked: resolving only the leaf accepts a root
// redirected by a symlink or a Windows junction.
export function assertNoLinks(file, { allowMissing = false } = {}) {
  const absolute = path.resolve(file);
  const volume = path.parse(absolute).root;
  let cursor = volume;
  for (const component of path.relative(volume, absolute).split(path.sep).filter(Boolean)) {
    cursor = path.join(cursor, component);
    let stat;
    try { stat = fs.lstatSync(cursor); } catch (error) {
      if (allowMissing && error.code === "ENOENT") return;
      throw error;
    }
    if (stat.isSymbolicLink()) throw new Error(`plugin path must not contain a symlink or junction: ${cursor}`);
  }
}

function entryPath(root, entry) {
  if (typeof entry !== "string" || entry.length === 0 || Buffer.byteLength(entry, "utf8") > 512
      || /[\x00-\x1f\x7f:]/.test(entry) || /^[\\/]/.test(entry)
      || entry.split(/[\\/]/).some((part) => !part || part === "." || part === ".." || /[. ]$/.test(part))) {
    throw new Error("plugin entry must be a bounded contained relative path");
  }
  const absolute = path.resolve(root, ...entry.split(/[\\/]/));
  const relative = path.relative(root, absolute);
  if (!relative || relative.startsWith(`..${path.sep}`) || relative === ".." || path.isAbsolute(relative)) throw new Error("plugin entry escapes artifact root");
  if (["plugin.json", INVENTORY_FILE].includes(relative.toLowerCase())) throw new Error("plugin entry collides with package metadata");
  return { absolute, relative: relative.split(path.sep).join("/") };
}

export function validateArtifact(pluginRoot, platform = currentPlatform()) {
  if (!["windowsX64", "linuxX64"].includes(platform)) throw new Error("unsupported plugin platform");
  const root = path.resolve(pluginRoot);
  assertNoLinks(root);
  if (!fs.statSync(root).isDirectory()) throw new Error("plugin artifact root must be a directory");
  const manifestPath = path.join(root, "plugin.json");
  assertNoLinks(manifestPath);
  const manifestBytes = readBounded(manifestPath, 256 * 1024, "plugin manifest");
  const manifest = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifestBytes));
  if (manifest.manifestVersion !== 2 || manifest.engine !== "process") throw new Error("plugin artifact must use process manifest v2");
  if (typeof manifest.id !== "string" || manifest.id.length > 255 || !ID.test(manifest.id)) throw new Error("invalid plugin id");
  if (typeof manifest.version !== "string" || manifest.version.length > 128 || !VERSION.test(manifest.version)
      || manifest.version.split("+")[0].split("-").slice(1).join("-").split(".").some((part) => /^0\d+$/.test(part))) throw new Error("invalid plugin version");
  if (!SHA256.test(manifest.integrity?.sha256 ?? "")) throw new Error("invalid plugin sha256");
  const selectedEntry = manifest.entry?.[platform];
  // Rust's Path keeps backslashes as literal filename characters on Linux.
  // Reject them rather than staging a different path from the manifest.
  if (platform === "linuxX64" && typeof selectedEntry === "string" && selectedEntry.includes("\\")) throw new Error("Linux plugin entry must use forward slashes");
  const entry = entryPath(root, selectedEntry);
  assertNoLinks(entry.absolute);
  if (!fs.statSync(entry.absolute).isFile()) throw new Error("plugin entry must be a file");
  const executableBytes = readBounded(entry.absolute, 256 * 1024 * 1024, "plugin executable");
  if (sha256(executableBytes) !== manifest.integrity.sha256) throw new Error("plugin executable sha256 does not match manifest");
  const { uiBytes, ui } = readUi(root, manifest, entry.relative);
  if (!validateManifestSchema(manifest)) throw new Error("invalid plugin manifest schema");
  const artifact = { root, manifestBytes, executableBytes, ...(uiBytes ? { uiBytes } : {}), row: { id: manifest.id, version: manifest.version, platform,
    entry: entry.relative, sha256: manifest.integrity.sha256, manifestSha256: sha256(manifestBytes), ...(ui ? { ui } : {}) } };
  assertArtifactContents(artifact);
  return artifact;
}

export function validateSelection(artifacts) {
  const ids = new Set();
  for (const artifact of artifacts) {
    if (ids.has(artifact.row.id)) throw new Error(`duplicate plugin id: ${artifact.row.id}`);
    ids.add(artifact.row.id);
  }
  return artifacts.toSorted((a, b) => a.row.id.localeCompare(b.row.id));
}

export function renderTrust(rows) {
  const single = rows.length === 1;
  const indent = single ? "" : "    ";
  const declarations = rows.map((row) => {
    const platform = { windowsX64: "WindowsX64", linuxX64: "LinuxX64" }[row.platform];
    if (!platform || !SHA256.test(row.sha256) || !SHA256.test(row.manifestSha256)) throw new Error("invalid plugin trust row");
    const width = single ? 15 : 14;
    const arrays = [row.sha256, row.manifestSha256].map((hash) => {
      const bytes = hash.match(/../g).map((value) => `0x${value}`);
      const lines = [];
      for (let index = 0; index < bytes.length; index += width) lines.push(`${indent}        ${bytes.slice(index, index + width).join(", ")},`);
      return `${indent}    [\n${lines.join("\n")}\n${indent}    ],`;
    });
    return `${indent}(\n${indent}    ${JSON.stringify(row.id)},\n${indent}    ${JSON.stringify(row.version)},\n${indent}    Platform::${platform},\n${arrays.join("\n")}\n${indent})${single ? "" : ","}`;
  });
  const body = rows.length === 0 ? "" : single ? declarations[0] : `\n${declarations.join("\n")}\n`;
  return `// Generated by tools/build/build-bundled-plugins.mjs.\nuse crate::plugins::manifest::Platform;\n\npub const GENERATED_PLUGIN_TRUST: &[(&str, &str, Platform, [u8; 32], [u8; 32])] = &[${body}];\n`;
}

export function stageArtifacts({ targetRoot, profile, artifacts }) {
  if (!["debug", "release"].includes(profile)) throw new Error("invalid build profile");
  const selected = validateSelection(artifacts);
  const output = path.resolve(targetRoot, profile, "bundled-plugins");
  assertNoLinks(output, { allowMissing: true });
  for (const artifact of selected) {
    const relative = path.relative(output, artifact.root);
    if (relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative))) throw new Error("plugin input must be outside the generated bundled directory");
  }
  fs.mkdirSync(path.dirname(output), { recursive: true });
  for (const entry of fs.existsSync(output) ? fs.readdirSync(output, { withFileTypes: true }) : []) {
    assertNoLinks(path.join(output, entry.name));
    if (entry.name !== INVENTORY_FILE && (!entry.isDirectory() || !ID.test(entry.name))) throw new Error(`unexpected file in generated bundled directory: ${entry.name}`);
  }
  const inventory = { schemaVersion: 1, plugins: selected.map((artifact) => artifact.row) };
  const staging = fs.mkdtempSync(path.join(path.dirname(output), ".bundled-plugins-stage-"));
  let backup;
  try {
    for (const artifact of selected) {
      const pluginRoot = path.join(staging, artifact.row.id);
      const binary = entryPath(pluginRoot, artifact.row.entry).absolute;
      fs.mkdirSync(path.dirname(binary), { recursive: true });
      fs.writeFileSync(path.join(pluginRoot, "plugin.json"), artifact.manifestBytes);
      fs.writeFileSync(binary, artifact.executableBytes, { mode: 0o755 });
      if (artifact.row.ui) {
        const uiPath = entryPath(pluginRoot, artifact.row.ui.entry).absolute;
        fs.mkdirSync(path.dirname(uiPath), { recursive: true });
        fs.writeFileSync(uiPath, artifact.uiBytes);
      }
    }
    fs.writeFileSync(path.join(staging, INVENTORY_FILE), `${JSON.stringify(inventory, null, 2)}\n`);
    readInventory(staging, selected[0]?.row.platform);
    // Preserve the prior generation, including omitted artifacts, outside the
    // discovery root. A failed write never modifies it; a failed swap restores it.
    if (fs.existsSync(output)) {
      assertNoLinks(output);
      const backupRoot = fs.mkdtempSync(path.join(path.dirname(output), ".bundled-plugins-backup-"));
      backup = path.join(backupRoot, "previous");
      fs.renameSync(output, backup);
    }
    try { fs.renameSync(staging, output); } catch (error) {
      if (backup) {
        try { fs.renameSync(backup, output); } catch (restoreError) {
          throw new AggregateError([error, restoreError], `plugin output restoration failed; previous generation is at ${backup}`);
        }
      }
      throw error;
    }
    if (backup) console.log(`[bundled-plugin] previous generation retained at ${backup}`);
  } finally {
    if (fs.existsSync(staging)) {
      assertNoLinks(staging);
      fs.rmSync(staging, { recursive: true, force: true });
    }
  }
  return inventory;
}

function assertArtifactContents(artifact) {
  const entries = [artifact.row.entry, ...(artifact.row.ui ? [artifact.row.ui.entry] : [])];
  const allowed = new Set(["plugin.json", ...entries]);
  for (const item of entries) {
    const components = item.split("/");
    for (let index = 1; index < components.length; index += 1) allowed.add(components.slice(0, index).join("/"));
  }
  const visit = (relative = "") => {
    for (const entry of fs.readdirSync(path.join(artifact.root, relative), { withFileTypes: true })) {
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      if (!allowed.has(child)) throw new Error(`plugin directory contains files outside the current inventory: ${artifact.row.id}/${child}`);
      assertNoLinks(path.join(artifact.root, child));
      if (entry.isDirectory()) visit(child);
    }
  };
  visit();
}

export function readInventory(bundledRoot, platform = currentPlatform()) {
  assertNoLinks(bundledRoot);
  const inventoryPath = path.join(bundledRoot, INVENTORY_FILE);
  assertNoLinks(inventoryPath);
  const inventory = JSON.parse(fs.readFileSync(inventoryPath, "utf8"));
  if (inventory.schemaVersion !== 1 || !Array.isArray(inventory.plugins) || inventory.plugins.length === 0) throw new Error("missing or invalid bundled plugin inventory");
  const artifacts = validateSelection(inventory.plugins.map((row) => {
    if (typeof row.id !== "string" || row.id.length > 255 || !ID.test(row.id)) throw new Error("invalid inventory plugin id");
    const artifact = validateArtifact(path.join(bundledRoot, row.id), platform);
    if (JSON.stringify(row) !== JSON.stringify(artifact.row)) throw new Error(`stale plugin inventory: ${row.id}`);
    assertArtifactContents(artifact);
    return artifact;
  }));
  const actual = fs.readdirSync(bundledRoot).filter((name) => name !== INVENTORY_FILE).sort();
  if (JSON.stringify(actual) !== JSON.stringify(artifacts.map((artifact) => artifact.row.id).sort())) throw new Error("bundled plugin directory contains artifacts outside the current inventory");
  return inventory;
}

export function assertSelectedInventory(inventory, externalArtifacts) {
  const selected = validateSelection(externalArtifacts);
  if (selected.some((artifact) => [FAKE_PLUGIN_ID, PPM_PLUGIN_ID].includes(artifact.row.id))) throw new Error("duplicate plugin id in default selection");
  const expected = [FAKE_PLUGIN_ID, PPM_PLUGIN_ID, ...selected.map((artifact) => artifact.row.id)].sort();
  if (JSON.stringify(inventory.plugins.map((row) => row.id).sort()) !== JSON.stringify(expected)) throw new Error("bundled inventory does not match explicitly selected plugins");
  for (const artifact of selected) {
    if (JSON.stringify(inventory.plugins.find((row) => row.id === artifact.row.id)) !== JSON.stringify(artifact.row)) throw new Error(`selected artifact changed: ${artifact.row.id}`);
  }
}
