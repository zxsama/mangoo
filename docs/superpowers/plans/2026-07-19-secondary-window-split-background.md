# Secondary Window Split Background Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the standalone Settings and Plugins window bodies a shared dark-left/light-to-dark-right material treatment without changing their titlebars or drawing a vertical divider.

**Architecture:** Derive semantic split-pane fill variables from the existing side-material preference in `interfaceStyle.ts`, then consume those variables in the two existing body layouts. Settings maps its navigation and main columns directly; Plugins groups its actions, status, errors, and list into one semantic sidebar so the right detail pane can use the same uninterrupted gradient contract.

**Tech Stack:** React 18, TypeScript, CSS, Electron BrowserWindow, existing Liquid Glass primitives, Node validation scripts.

---

### Task 1: Guard the split-pane visual contract

**Files:**
- Modify: `tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Add failing assertions for the approved structure and CSS contract**

Add checks beside the existing secondary-window assertions that require:

```js
for (const variable of [
  "--secondary-window-pane-dark",
  "--secondary-window-pane-light",
  "--secondary-window-pane-end"
]) {
  if (!interfaceStyle.includes(variable)) {
    fail(`interface style must derive secondary-window split material variable ${variable}`);
  }
}

if (!pluginsView.includes('className="plugins-sidebar"')) {
  fail("PluginsView must group plugin controls and list into the dark secondary-window sidebar");
}

const secondaryTitlebarBlocks = cssBlocksForExactSelector(
  stylesForChecks,
  ".secondary-window-titlebar"
);
if (
  !secondaryTitlebarBlocks.some(
    (block) => latestDeclarationValue(block, "border-bottom") === "1px solid var(--line)"
  )
) {
  fail("secondary-window titlebar must retain its existing bottom divider");
}

const settingsNavBlocks = cssBlocksForExactSelector(stylesForChecks, ".settings-dialog-nav");
if (
  !settingsNavBlocks.some(
    (block) =>
      latestDeclarationValue(block, "border-right") === "0" &&
      latestDeclarationValue(block, "background") === "var(--secondary-window-pane-dark)"
  )
) {
  fail("settings navigation must be the dark pane with no vertical divider");
}

const settingsMainBlocks = cssBlocksForExactSelector(stylesForChecks, ".settings-dialog-main");
if (
  !settingsMainBlocks.some((block) => {
    const background = latestDeclarationValue(block, "background") ?? "";
    return (
      background.includes("linear-gradient") &&
      background.includes("var(--secondary-window-pane-light)") &&
      background.includes("var(--secondary-window-pane-end)")
    );
  })
) {
  fail("settings content must use the shared light-to-dark body gradient");
}

const pluginSidebarBlocks = cssBlocksForExactSelector(stylesForChecks, ".plugins-sidebar");
if (
  !pluginSidebarBlocks.some(
    (block) => latestDeclarationValue(block, "background") === "var(--secondary-window-pane-dark)"
  )
) {
  fail("plugins sidebar must use the shared dark-pane material");
}
```

- [ ] **Step 2: Run the validator and confirm it fails for the missing contract**

Run:

```powershell
npm run check:ui-design
```

Expected: FAIL because the semantic variables, `.plugins-sidebar`, and pane backgrounds do not exist yet.

- [ ] **Step 3: Commit the failing visual contract**

```powershell
git add tools/checks/validate-ui-design.mjs
git commit -m "test(ui): guard secondary window split material"
```

### Task 2: Derive split-pane colors from interface style

**Files:**
- Modify: `apps/web/src/features/settings/interfaceStyle.ts`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: Add fallback semantic variables to the root CSS token block**

Add defaults next to the existing `--glass-side-*` variables:

```css
--secondary-window-pane-dark: rgba(8, 12, 16, 0.56);
--secondary-window-pane-light: rgba(8, 12, 16, 0.2);
--secondary-window-pane-end: rgba(8, 12, 16, 0.48);
```

- [ ] **Step 2: Derive the variables from the existing side-material preference**

Inside `interfaceStyleToCssVariables`, add:

```ts
const secondaryWindowPaneDark = colorWithAlpha(
  normalized.sideOverlayColor,
  0.56 * normalized.sideOverlayStrength * normalized.sideOpacity
);
const secondaryWindowPaneLight = colorWithAlpha(
  normalized.sideOverlayColor,
  0.2 * normalized.sideOverlayStrength * normalized.sideOpacity
);
const secondaryWindowPaneEnd = colorWithAlpha(
  normalized.sideOverlayColor,
  0.48 * normalized.sideOverlayStrength * normalized.sideOpacity
);
```

Return the values with the existing CSS variable map:

```ts
"--secondary-window-pane-dark": secondaryWindowPaneDark,
"--secondary-window-pane-light": secondaryWindowPaneLight,
"--secondary-window-pane-end": secondaryWindowPaneEnd,
```

These are derived tokens only; do not add new persisted preferences or controls.

- [ ] **Step 3: Map the Settings columns to the shared material**

Update the existing selectors in `styles.css`:

```css
.settings-dialog-nav {
  border-right: 0;
  background: var(--secondary-window-pane-dark);
}

.settings-dialog-main {
  background: linear-gradient(
    108deg,
    var(--secondary-window-pane-light) 0%,
    var(--glass-side-fill) 52%,
    var(--secondary-window-pane-end) 100%
  );
}
```

Keep every existing sizing, spacing, scrolling, and navigation-state declaration.

- [ ] **Step 4: Run focused style and TypeScript checks**

Run:

```powershell
npm --workspace @megle/web run typecheck
npm run check:ui-design
```

Expected: TypeScript PASS. UI design still FAILS only because the Plugins structure and styling from Task 3 are not present.

- [ ] **Step 5: Commit the shared tokens and Settings mapping**

```powershell
git add apps/web/src/features/settings/interfaceStyle.ts apps/web/src/styles.css
git commit -m "feat(ui): add split material to settings window"
```

### Task 3: Make Plugins use the same two-pane structure

**Files:**
- Modify: `apps/web/src/features/plugins/PluginsView.tsx`
- Modify: `apps/web/src/styles.css`

- [ ] **Step 1: Group Plugins controls and list into a semantic sidebar**

Keep all existing conditions and handlers, but change the body hierarchy to:

```tsx
<div className="plugins-body">
  <div className="plugins-layout">
    <aside className="plugins-sidebar">
      <div className="plugins-body-actions">
        <div className="plugins-status-summary">
          {totalCount === 0
            ? "No plugins discovered"
            : `${totalCount} registered · ${enabledCount} enabled${
                invalidCount > 0 ? ` · ${invalidCount} invalid` : ""
              }`}
        </div>
        <button
          className="plugins-rescan"
          onClick={() => void runDiscover()}
          type="button"
          disabled={discovering}
          aria-label="Re-scan plugin folder"
        >
          {discovering ? <Loader2 className="spin" size={14} /> : <RefreshCw size={14} />}
          <span>{discovering ? "Scanning…" : "Re-scan"}</span>
        </button>
      </div>
      {plugins.error ? (
        <div className="plugins-error" role="alert">
          <AlertTriangle aria-hidden="true" size={14} />
          <span>{plugins.error}</span>
        </div>
      ) : null}
      {plugins.lastDiscovery && plugins.lastDiscovery.errors.length > 0 ? (
        <div className="plugins-discovery-errors" role="status">
          <div className="plugins-discovery-summary">
            Last scan: {plugins.lastDiscovery.discovered} discovered,{" "}
            {plugins.lastDiscovery.errors.length} error
            {plugins.lastDiscovery.errors.length === 1 ? "" : "s"}
          </div>
          <ul className="plugins-discovery-error-list">
            {plugins.lastDiscovery.errors.map((entry, index) => (
              <li key={`${entry.manifestPath}-${index}`}>
                <code title={entry.manifestPath}>{entry.manifestPath}</code>
                <span>{entry.message}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <div className="plugins-list" role="list">
        {plugins.loading && totalCount === 0 ? (
          <div aria-label="Refreshing plugins" className="empty-panel empty-panel-busy" role="status">
            <MediaLoadingRing size={18} />
          </div>
        ) : totalCount === 0 ? (
          <div className="plugins-empty">
            <p className="plugins-empty-title">No plugins discovered yet.</p>
            <p className="plugins-empty-hint">
              Drop a plugin folder under{" "}
              <code className="dialog-code">MEGLE_PLUGINS_DIR</code> and click Re-scan.
            </p>
          </div>
        ) : (
          plugins.plugins.map((plugin) => (
            <PluginCard
              busy={plugins.busyPluginIds.has(plugin.id)}
              key={plugin.id}
              onSelect={() => plugins.selectPlugin(plugin.id)}
              onToggleEnabled={() => {
                if (plugin.enabled) void plugins.disable(plugin.id);
                else void plugins.enable(plugin.id);
              }}
              plugin={plugin}
              selected={plugins.selectedPluginId === plugin.id}
            />
          ))
        )}
      </div>
    </aside>
    <LiquidGlassSurface
      as="div"
      className="plugins-detail-pane"
      interactive
      scrollable
      tone="panel"
    >
      {selected ? (
        <PluginDetail
          busy={plugins.busyPluginIds.has(selected.id)}
          onRemove={() => void plugins.remove(selected.id)}
          plugin={selected}
        />
      ) : (
        <div className="plugins-detail-empty">
          {totalCount === 0
            ? "Once a plugin is registered, its details will appear here."
            : "Select a plugin to inspect capabilities, permissions, and manifest details."}
        </div>
      )}
    </LiquidGlassSurface>
  </div>
</div>
```

No plugin data flow, loading state, action, selection, or accessibility label changes.

- [ ] **Step 2: Style the Plugins body as an uninterrupted split surface**

Replace the padded card-like layout with:

```css
.plugins-body {
  grid-row: 1 / -1;
  min-height: 0;
  overflow: hidden;
  background: linear-gradient(
    108deg,
    var(--secondary-window-pane-light) 0%,
    var(--glass-side-fill) 52%,
    var(--secondary-window-pane-end) 100%
  );
}

.plugins-layout {
  display: grid;
  height: 100%;
  grid-template-columns: minmax(280px, 360px) minmax(0, 1fr);
  gap: 0;
  min-height: 0;
  overflow: hidden;
}

.plugins-sidebar {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  min-height: 0;
  padding: 16px;
  background: var(--secondary-window-pane-dark);
  overflow: hidden;
}

.plugins-detail-pane {
  --glass-fill: transparent;
  --glass-border-current: transparent;
  min-width: 0;
  min-height: 0;
  padding: 16px 18px;
  border: 0;
  border-radius: 0;
  background: transparent;
  box-shadow: none;
  overflow: auto;
}

.plugins-window .plugins-detail-pane::after {
  border: 0;
  box-shadow: none;
}
```

Make `.plugins-list` flex to fill the sidebar and keep its existing scroll behavior. Do not add a border, box shadow, or gap between the sidebar and details.

- [ ] **Step 3: Run focused checks**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: PASS.

- [ ] **Step 4: Commit the Plugins mapping**

```powershell
git add apps/web/src/features/plugins/PluginsView.tsx apps/web/src/styles.css
git commit -m "feat(ui): align plugins window split material"
```

### Task 4: Verify the desktop material and interaction behavior

**Files:**
- Modify only if verification exposes a defect in the files already named above.

- [ ] **Step 1: Run all required automated checks**

```powershell
npm run check:web
npm run check:desktop
npm run check:ui-design
```

Expected: all PASS.

- [ ] **Step 2: Run the OS-backdrop desktop visual harness**

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'
node .tmp\visual-check\desktop-ui-regression.mjs
Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

Expected: PASS with Settings and Plugins opening through the normal secondary-window lifecycle.

- [ ] **Step 3: Perform the real-window interaction pass**

Verify on the displayed Windows desktop:

1. Settings and Plugins titlebars are unchanged and retain their bottom line.
2. Settings navigation and Plugins sidebar are dark.
3. Both right panes begin lighter and fade to dark toward the right edge.
4. No vertical divider, border seam, or detail-card outline separates the panes.
5. Side-material tint, opacity, blur, and saturation changes update both windows.
6. Settings navigation, plugin scrolling, plugin selection, rescan, close, drag, and reveal behavior remain functional.

- [ ] **Step 4: Run the final repository hygiene checks**

```powershell
git diff --check
git status --short
```

Expected: no whitespace errors. Existing unrelated `Cargo.toml` and untracked probe files remain outside these commits.
