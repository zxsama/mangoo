import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { assertNoLinks, currentPlatform, FAKE_PLUGIN_ID, PPM_PLUGIN_ID, parsePluginArgs, renderTrust, sha256, stageArtifacts, validateArtifact, validateSelection } from "./plugin-artifacts.mjs";

const root = process.cwd();
const options = parsePluginArgs(process.argv.slice(2));
const profile = options.releaseProfile ? "release" : "debug";
const platform = currentPlatform();
const targetRoot = path.resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const dataRelative = path.relative(path.join(root, ".data"), targetRoot);
if (!dataRelative.startsWith(`..${path.sep}`) && dataRelative !== ".." && !path.isAbsolute(dataRelative)) throw new Error("bundled build output must not use the production data directory");
const external = validateSelection(options.externalPlugins.map((artifact) => validateArtifact(artifact, platform)));
if (external.some((artifact) => [FAKE_PLUGIN_ID, PPM_PLUGIN_ID].includes(artifact.row.id))) throw new Error("duplicate built-in plugin id");
execFileSync("cargo", ["build", ...(options.releaseProfile ? ["--release"] : []), "-p", "mangoo-fake-provider", "-p", "mangoo-ppm-decoder"], {
  cwd: root, stdio: "inherit", shell: false, windowsHide: true,
});
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "mangoo-bundle-"));
let inventory;
try {
  const defaults = [];
  for (const [id, executable] of [[FAKE_PLUGIN_ID, "mangoo-fake-provider"], [PPM_PLUGIN_ID, "mangoo-ppm-decoder"]]) {
    const binary = platform === "windowsX64" ? `${executable}.exe` : executable;
    const built = path.join(targetRoot, profile, binary);
    assertNoLinks(built);
    const bytes = fs.readFileSync(built);
    const manifestTemplate = fs.readFileSync(path.join(root, "bundled-plugins", id, "plugin.template.json"), "utf8");
    const directory = path.join(scratch, id);
    fs.mkdirSync(path.join(directory, "bin"), { recursive: true });
    fs.writeFileSync(path.join(directory, "bin", binary), bytes);
    fs.writeFileSync(path.join(directory, "plugin.json"), manifestTemplate.replaceAll("__SHA256__", sha256(bytes)));
    defaults.push(validateArtifact(directory, platform));
  }
  inventory = stageArtifacts({ targetRoot, profile, artifacts: [...defaults, ...external] });
} finally {
  assertNoLinks(scratch);
  fs.rmSync(scratch, { recursive: true, force: true });
}
const generatedTrustPath = path.join(root, "crates", "core", "src", "plugins", "runtime", "generated_trust.rs");
const generatedTrust = renderTrust(inventory.plugins);
if (!fs.existsSync(generatedTrustPath) || fs.readFileSync(generatedTrustPath, "utf8") !== generatedTrust) fs.writeFileSync(generatedTrustPath, generatedTrust);
for (const plugin of inventory.plugins) console.log(`[bundled-plugin] ${plugin.id} ${plugin.version} ${plugin.platform} ${plugin.sha256}`);
if (options.release) {
  execFileSync("cargo", ["build", "--release", "-p", "mangoo-core"], { cwd: root, stdio: "inherit", shell: false, windowsHide: true });
}
