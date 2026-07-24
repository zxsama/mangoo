# Repository Guidelines

## Project Structure & Module Organization

Megle is a Windows-first, contract-first local media browser. The main stack is Electron desktop, React + TypeScript + Vite UI, Rust + Axum Core, and SQLite WAL.

- `apps/desktop/`: Electron shell, Windows desktop adapter, window lifecycle, native dialogs, session-token handoff, and Core process lifecycle.
- `apps/web/`: React UI shared by desktop and future Web/Docker modes. UI code must talk to Core through `@megle/core-client` wrappers/hooks, not direct filesystem access.
- `contracts/core-api/`: OpenAPI contract for Core HTTP endpoints, DTO names, and status codes.
- `contracts/plugins/`: plugin manifest and permission contract.
- `crates/core/`: single Phase 1 Rust Core crate. Keep API, DB, roots, scan, thumbnails, fsops, plugins, watch, and tasks as internal modules until boundaries are proven.
- `packages/core-client/`: TypeScript Core API client/types boundary.
- Multi-library: a library = one `.sqlite` file (with N roots inside); switching restarts Core at a new `MEGLE_DB_PATH` reusing the same port+token (no renderer reload). Registry in `apps/desktop/src/library-registry.ts` → `userData/libraries.json`; bridge/types in `apps/web/src/core/desktop.ts` + `libraryTypes.ts`. Core is DB-path-driven and unchanged. See CLAUDE.md → "Multi-Library".
- `tools/checks/`: fast structure, contract, schema, desktop, web, and UI-design checks.
- `tools/bench/`: heavier benchmark harnesses and generated-data tooling.
- `docs/`: durable architecture, product, design, plans, benchmark reports, and release documentation.

## File Placement Rules

Keep the repository root clean. Root-level files are only for repository entrypoints or tool-required files such as `AGENTS.md`, `README.md`, `package.json`, `Cargo.toml`, Docker files, and launch helper scripts.

- Put durable documentation under `docs/`.
- Put implementation plans and design work under `docs/superpowers/plans/` or `docs/superpowers/specs/`.
- Put fast validation tooling under `tools/checks/`.
- Put benchmark and reproducible experiment tooling under `tools/bench/`.
- Put temporary logs, screenshots, generated visual-check output, local databases, and scratch files under ignored folders such as `.tmp/`, `tmp/`, `.data/`, `.serena/`, or tool-specific ignored result directories.
- Do not commit `node_modules/`, `target/`, build output, SQLite databases, transient logs, benchmark result dumps, Serena cache/log files, or generated thumbnail/cache artifacts.

## Build, Test, and Development Commands

Run from the repository root.

- `npm run dev`: start Vite, build desktop main, and launch Electron/Core dev harness.
- `npm test`: full verification gate: structure, contracts, Core client, desktop, web, UI design, SQLite schema, and Rust checks.
- `npm run check:structure`: required directory, script, and boundary checks.
- `npm run check:core-api`: validate OpenAPI/Core API alignment.
- `npm run check:core-client`: validate `@megle/core-client` contract alignment, typecheck, and tests.
- `npm run check:desktop`: desktop boundary validation and TypeScript typecheck.
- `npm run check:web`: web boundary validation and TypeScript typecheck.
- `npm run check:ui-design`: acrylic/frosted-glass / desktop shell / UI design guardrails.
- `npm run check:schema`: apply migrations to a temporary SQLite DB and verify schema/indexes.
- `npm run check:rust`: `cargo fmt --all --check` and `cargo test --workspace` when Rust is available.

Workspace-scoped commands:

- `npm --workspace @megle/web run dev|build|typecheck|preview`
- `npm --workspace @megle/desktop run build|typecheck`
- `npm --workspace @megle/core-client run check|typecheck|test`
- `cargo test -p megle-core <test_name>`
- `cargo run -p megle-core` with `MEGLE_SESSION_TOKEN` set.

Desktop glass regressions require the explicit OS-backdrop harness:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

## Coding Style & Naming Conventions

- Use TypeScript/React patterns already present in `apps/web`; keep UI data access behind hooks and `@megle/core-client`.
- Do not add raw `fetch` calls, duplicate Core DTOs, or Node filesystem imports in `apps/web`.
- Keep Electron-only work in `apps/desktop`; do not bypass Core HTTP boundaries from feature UI code.
- Rust code should follow `cargo fmt` and stay inside `crates/core` modules unless a real split is planned and validated.
- New API paths start in `contracts/core-api/openapi.yaml`; new plugin capabilities start in `contracts/plugins/manifest.schema.json`; new tables/indexes require numbered SQLite migrations.
- Prefer small, scoped changes over speculative abstractions. Do not weaken validation scripts to make a change pass.

## UI And Desktop Shell Rules

The approved UI direction is documented in `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md`.

- Preserve the Eagle-like information architecture while keeping Megle visually distinct.
- Keep the Library workspace aligned with the Eagle-inspired browsing model now present in the app: titlebar layout switching, subfolder strip + content split in the center pane, and preview-toolbar parity for layout controls.
- Library layout modes are first-class product behavior. Treat `adaptive`, `waterfall`, `grid`, and `list` as a shared contract across App state, titlebar controls, MediaGrid geometry, scroll restoration, keyboard navigation, and visible/ahead thumbnail priority logic.
- Use one layered acrylic frosted-glass design language across Library, Settings, Plugins, Tasks, menus, dialogs, context menus, inspectors, and desktop chrome.
- Windows desktop must use real behind-window transparency/acrylic where required; do not fake it with a full-window renderer backing plate.
- Acrylic and frosted-glass materials belong on chrome, control, floating, and overlay layers. Media grids, preview canvases, and heavy content surfaces should remain sharp and stable.
- All interactive elements share ONE hover highlight: a single cursor-centred circular glow driven globally by `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx` (`updateInteractiveAffordancePointers`), styled on `[data-interactive-pointer-target]::before/::after` in `styles.css`, with params in `features/settings/interfaceStyle.ts`. Never add a per-element `:hover` border/background or a second glow subsystem. Opt an element out with `data-pointer-glow="off"`. Selection/active state uses a background fill, not a border ring. See CLAUDE.md → "Unified Pointer-Highlight System" for the full contract (the mask-composite corner-AA limitation and the O(near-cursor) driver design are documented there).
- Popup and dialog work must use the shared glass popup surface conventions rather than ad hoc transparency. Titlebar menus, task drawers, recent-op drawers, and modal surfaces should sit above shell chrome with blur, fill, and z-index behavior that stays consistent across the app.
- Shell, pointer highlight, titlebar drag, acrylic, rounded-corner, and dialog material changes must be guarded by `npm run check:ui-design` and the desktop visual regression harness.
- CDP `Page.captureScreenshot` does not reflect real on-display anti-aliasing (screenshots look smoother than the physical compositor at dpr=1). For AA-sensitive UI, verify on the real window with the user; screenshots stay reliable for layout, state, and whether elements rendered. Electron is pinned to `43.1.0` to keep acrylic on drag-maximize — do not downgrade without re-testing.

## Testing Guidelines

Code and tests move together.

- New directory: update `docs/project-structure.md` and `tools/checks/validate-structure.mjs`.
- New API path: update OpenAPI first, then client/Core/UI, then checks.
- New table or hot-path index: add a migration and update `tools/checks/validate_sqlite_schema.py`.
- UI boundary changes: run `npm run check:web` and relevant focused UI checks.
- Layout-mode or popup-surface changes: verify `npm run check:web` and perform a real desktop interaction pass that covers layout switching, preview return, subfolder-strip interaction, and compact popover/dialog layering.
- Desktop shell/material changes: run `npm run check:desktop`, `npm run check:ui-design`, and the OS-backdrop visual harness.
- Broad or release-facing changes: run `npm test` before claiming completion.

## Serena MCP / LSP Semantic Navigation

Serena is installed for Codex as the LSP-backed MCP server for this machine.

- Installed package: `serena-agent==1.5.3`.
- CLI path: `C:\Users\84460\.local\bin\serena.exe`.
- Codex MCP registration: `serena start-mcp-server --context=codex --project-from-cwd` in `C:\Users\84460\.codex\config.toml`.
- Project languages: `typescript` and `rust`.
- Project-local Serena data lives in `.serena/` and is intentionally ignored because it contains local config, caches, memories, and health-check logs.

Use Serena MCP tools first for semantic code understanding when they are available in the Codex tool list:

- `get_symbols_overview`
- `find_symbol`
- `find_referencing_symbols`
- `find_declaration`
- `find_implementations`
- `get_diagnostics_for_file`
- `rename_symbol`
- `safe_delete_symbol`

Use `rg` for literal text search, docs/config lookup, and cases where LSP returns no useful result. For Rust code, Serena starts `rust-analyzer`; for TypeScript/React, Serena installs and uses `typescript-language-server` through its own language-server cache while still using the workspace TypeScript version.

Useful local Serena commands:

```powershell
serena project create . --name Megle --language typescript --language rust
serena project health-check .
serena project index .
```

After first-time MCP setup, restart Codex or open a new Codex session so the newly registered Serena MCP server is loaded.

## Commit & Pull Request Guidelines

Use Git. Keep commits focused and use short imperative or conventional-style messages, for example `fix(web): preserve grid scroll state` or `Stabilize desktop glass shell`.

Before committing:

- Check `git status --short` and avoid mixing unrelated user changes.
- Run the narrowest meaningful verification for the touched area.
- For large UI/desktop changes, rerun the critical checks immediately before commit.
- Do not amend existing commits unless explicitly requested.

PRs or review notes should include scope, touched subsystems, verification commands/results, and screenshots or visual evidence for UI/acrylic changes.

## Agent-Specific Instructions

- Read nearby code, contracts, and validation scripts before editing.
- Preserve user changes already present in the worktree; do not revert unrelated dirty files.
- Prefer global primitives and validators over local one-off CSS/logic patches, especially for acrylic/frosted-glass materials, shell, titlebar, dialog, and pointer-highlight work.
- Do not claim success without fresh verification evidence. If checks are skipped or blocked, say so directly.
- When subagent workflow is explicitly requested, use one subagent at a time unless the user explicitly allows parallel dispatch.
- For desktop acrylic/interface work, treat `npm run check:ui-design`, `npm run check:web`, `npm run check:desktop`, and the OS-backdrop visual harness as the expected verification set.
