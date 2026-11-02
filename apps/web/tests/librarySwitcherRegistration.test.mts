import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { it } from "node:test";
import { createServer } from "./viteTestServer.mts";

it("mounted Library switcher preserves safe registration and searchable appearance flows", async () => {
  const server = await createServer({ root: fileURLToPath(new URL("../", import.meta.url)), logLevel: "error", server: { host: "127.0.0.1", port: 0 } });
  try {
    await server.listen();
    const address = server.httpServer?.address();
    assert.ok(address && typeof address === "object");
    const electron = (await import("electron")).default as unknown as string;
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    const output = await new Promise<string>((resolve, reject) => {
      const child = spawn(electron, [fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)), `http://127.0.0.1:${address.port}/tests/fixtures/library-switcher-lifecycle.html?followup`], { env, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "";
      let stderr = "";
      const timeout = setTimeout(() => { child.kill(); reject(new Error(`switcher timed out: ${stderr}`)); }, 150000);
      child.stdout.on("data", (chunk) => { stdout += String(chunk); });
      child.stderr.on("data", (chunk) => { stderr += String(chunk); });
      child.on("error", reject);
      child.on("exit", () => { clearTimeout(timeout); resolve(stdout || stderr); });
    });
    const line = output.split("\n").find((value) => value.startsWith("OVERLAY_LIFECYCLE_RESULT:"));
    assert.ok(line, output);
    const result = JSON.parse(line.slice("OVERLAY_LIFECYCLE_RESULT:".length));
    assert.equal(result.ok, true, result.error);
    assert.ok(result.followup.length >= 24, JSON.stringify(result));
    for (const check of [
      "Database customization opens through its native pointer disclosure before choosing a directory",
      "New creation submits pure white and default glyph without appearance edits",
      "Visible type groups follow catalog order and retain a separate current group",
      "Each visible heading contains its exact stable catalog members without duplicate choices",
      "Groups keep one radio Tab entry, noninteractive headings and one shared scroll area",
      "Unknown saved choice is selected and keyboard reachable in its current group",
      "Grouped icons retain the fixed compact scroll viewport",
      "ArrowDown clamps to the nearest available partial-row column at 248px and 1.25 zoom",
      "ArrowUp preserves the actual partial-row column at 248px and 1.25 zoom",
      "ArrowUp follows the previous row after clicking its last column at 248px and 1.25 zoom",
      "ArrowUp stays in the rendered column after pointer hover lifts a cell at 248px and 1.25 zoom",
      "ArrowDown crosses a type heading at 248px and 1.25 zoom",
      "ArrowRight follows DOM order across type groups at 248px and 1.25 zoom",
      "ArrowLeft reverses a type-group transition at 248px and 1.25 zoom",
      "Keyboard reaches and reveals a new final-row glyph with draft preview only",
      "Horizontal navigation wraps from the last type to the first",
      "Horizontal navigation wraps from the first type to the last",
      "Home reaches the first visible identity across all types",
      "Search results show multiple matching types together",
      "Empty search shows a status without empty headings",
      "Category filters to one visible type heading",
      "Changing color preserves an unknown saved icon key",
      "Real global shortcuts leave root Tab navigation to the row",
      "Enter activates the independent more action",
      "Real global shortcuts leave menu Tab navigation to the overlay",
      "Real global shortcuts leave appearance Tab navigation to the overlay",
      "Tab resumes the Library shortcut after popup closes",
      "Already handled keyboard events do not reach global shortcuts",
      "Whole row owns pointer highlight over name",
      "Whole row owns pointer highlight over more",
      "Pointer edge spans entire row over more",
      "Tab reaches independent more action",
      "Keyboard and pointer share one row boundary",
      "More keyboard focus emphasizes foreground",
      "Keyboard more opens menu without activating Library",
      "Menu Escape restores independent more focus",
      "Main keyboard focus emphasizes name within row boundary",
      "Inactive Library more never triggers activation",
      "Reset is absent from the more menu",
      "Reset belongs to the appearance editor",
      "Appearance save disables every choice and reset",
      "Busy apply prevents duplicate submissions and dismissal",
      "Reset applies explicit default glyph and white together",
      "Open dialog shares padded creation typography and layout",
      "Open dialog keeps aligned shared glass actions with one thin boundary",
      "Open dialog uses the shared full-width single-boundary field and regular body label",
      "Forget dialog shares padded creation typography and layout",
      "Forget dialog keeps aligned shared glass actions with one thin boundary",
      "Known Core errors give Chinese recovery guidance linked to the path field",
      "Inactive forget preserves count, virtual slots and selection",
      "Active forget without replacement clears count and pagination state",
      "Action menu places each full-width item on a separate readable row",
      "Action menu uses transparent shared controls without native button bevels",
      "Selected white swatch has a visible dark check instead of color-only selection",
      "Selected swatch check is centered and visibly sized",
      "Settings removes a remotely forgotten row",
      "Settings registry refresh does not publish media context phases",
      "Settings ignores an older registry response",
      "Settings unsubscribes and retires pending registry results on unmount",
    ]) assert.ok(result.followup.includes(check), check);
  } finally { await server.close(); }
});
