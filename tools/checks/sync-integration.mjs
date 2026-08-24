import path from "node:path";
import process from "node:process";
import { createRunDirectory, environmentFacts, requiredFailures, root, runCommand, writeJson } from "./sync-release-lib.mjs";

function parseArgs(argv) {
  const options = { output: null, quick: false };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value === "--output" && argv[index + 1]) options.output = argv[++index];
    else if (value === "--quick") options.quick = true;
    else throw new Error(`unknown or incomplete argument: ${value}`);
  }
  return options;
}

const options = parseArgs(process.argv.slice(2));
const runDirectory = await createRunDirectory("integration", options.output);
const targetRoot = process.env.CARGO_TARGET_DIR ? path.resolve(root, process.env.CARGO_TARGET_DIR) : path.join(root, "target");
const cargoEnvironment = { MEGLE_SYNC_BUNDLED_PLUGINS_DIR: path.join(targetRoot, "debug", "bundled-plugins") };
const cargo = async (name, filter, extra = [], env = {}) => runCommand({
  name,
  command: "cargo",
  args: ["test", "-p", "megle-core", filter, "--", ...extra, "--nocapture"],
  env: { ...cargoEnvironment, ...env },
  runDirectory,
  timeoutMs: 30 * 60 * 1000,
});

const rows = [];
rows.push(await runCommand({
  name: "legacy-selector-adoption-absence",
  command: "node",
  args: ["tools/checks/sync-legacy-absence.mjs"],
  runDirectory,
}));
rows.push(await cargo("clean-managed-bootstrap", "library_manager::tests::clean_managed_bootstrap_reopens_current_schema"));
rows.push(await runCommand({
  name: "build-bundled-providers",
  command: "node",
  args: ["tools/build/build-bundled-plugins.mjs"],
  runDirectory,
  timeoutMs: 30 * 60 * 1000,
}));
rows.push(await cargo(
  "real-provider-two-device-third-restore",
  "library_manager::tests::real_provider_two_device_and_third_device_restore",
  ["--ignored"],
  { MEGLE_SYNC_BUNDLED_PLUGINS_DIR: path.join(targetRoot, "debug", "bundled-plugins") },
));
rows.push(await cargo("two-device-conflict-matrix", "sync::test_model::reducer_converges_for_permutations_and_duplicates"));
rows.push(await cargo("two-device-delete-restore", "sync::test_model::restore_only_observed_delete_tokens"));
rows.push(await cargo("two-device-convergence", "sync::test_model::hundred_thousand_shuffled_and_duplicated_events_converge"));
rows.push(await cargo("duplicate-out-of-order-intake", "sync::inbox::tests::gap_blocks_only_the_affected_actor_and_exact_duplicates_are_noop"));
rows.push(await cargo("checkpoint-selection", "sync::test_model::checkpoint_selection_uses_vector_coverage_not_creation_order"));
rows.push(await cargo("third-device-restore", "sync::persistence::tests::checkpoint_uses_external_bounded_media_indexes_and_restores_chunk_locations"));
rows.push(await cargo("media-publication-ordering", "sync::media::worker::tests::publication_worker_uploads_dependencies_before_releasing_event"));
rows.push(await cargo("multi-library-fairness", "sync::runtime::tests::fake_coordinator_is_fair_and_deduplicates_turns"));
rows.push(await cargo("unbind-generation-fence", "sync::coordinator::tests::cancelled_media_binding_remains_fenced_against_late_turns"));
rows.push(await runCommand({
  name: "fault-matrix",
  command: "node",
  args: ["tools/checks/sync-fault-matrix.mjs", "--output", path.join(runDirectory, "fault-matrix")],
  runDirectory,
  timeoutMs: 60 * 60 * 1000,
}));

const result = {
  schemaVersion: 1,
  kind: "sync-integration",
  runDirectory,
  quick: options.quick,
  environment: await environmentFacts(),
  rows,
  status: requiredFailures(rows).length === 0 ? "passed" : "failed",
};
await writeJson(path.join(runDirectory, "integration-result.json"), result);
console.log(`SYNC_INTEGRATION_JSON=${JSON.stringify(result)}`);
if (result.status !== "passed") process.exitCode = 1;
