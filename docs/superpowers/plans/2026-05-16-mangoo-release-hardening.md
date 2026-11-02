# Mangoo Release Hardening Plan

> **For agentic workers:** Implement task-by-task. Steps use `- [ ]` checkboxes.

**Goal:** Define and reach "complete product" — settings polish, onboarding/empty states, keyboard shortcuts, accessibility pass, regression pass, release checklist. Phase 10 closes the loop and produces a demoable, releasable build.

**Definition of complete product (frozen):**
1. Main workflows work — add root, browse real files, preview, rate/tag/note, search, rename/move/recycle.
2. Failure workflows work — corrupt file, missing ffmpeg, invalid manifest, scan cancellation, retry, recycle restore.
3. Dangerous actions are trustworthy — permanent delete confirmation matches count; rename validates server-side rules; move surfaces cross_root cleanly.
4. Design system covers the whole app — Library / Tasks / Plugins / Settings all share toolbar/sidebar/inspector/dialog/context-menu language.
5. Desktop build is demoable — `npm run dev` boots the full app on Windows; release Electron build packages and runs.

---

## File Structure

**Modify:**
- `D:/Mangoo/apps/desktop/src/main.ts` (production-ready window state, frameless chrome polish)
- `D:/Mangoo/apps/web/src/app/App.tsx` (onboarding empty state, settings page, shortcuts)
- `D:/Mangoo/apps/web/src/features/library/*` (empty state, no-roots state)
- `D:/Mangoo/apps/web/src/features/preview/PreviewPanel.tsx` (empty / failed / skipped polish)
- `D:/Mangoo/apps/web/src/features/tasks/TaskCenter.tsx` (final spacing / accessibility pass)
- `D:/Mangoo/apps/web/src/styles.css` (design system audit; no orphan classes; consistent spacing scale)
- `D:/Mangoo/CLAUDE.md` (final state snippet)
- `D:/Mangoo/docs/README.md` (point at the master plan as complete)
- `D:/Mangoo/.codex/memory.md` (mirror of CLAUDE.md final state, if still in use)

**Create:**
- `D:/Mangoo/apps/web/src/features/settings/SettingsView.tsx`
- `D:/Mangoo/apps/web/src/features/onboarding/OnboardingHero.tsx` (no-roots state)
- `D:/Mangoo/apps/web/src/features/shortcuts/useShortcuts.ts` (global keybinds)
- `D:/Mangoo/docs/release-checklist.md`

---

## Tasks

### Task 1 — Onboarding & empty states

- [ ] No-roots: `OnboardingHero` with "Add a folder to get started" button that focuses the existing root form.
- [ ] No-media-in-root: helpful state ("Empty folder — drop image or video files in <path>").
- [ ] No search results: "Nothing matched <query>. Clear filters?" inline.

### Task 2 — Settings page

- [ ] Replace App.tsx Settings placeholder with `SettingsView`:
  - Thumbnail cache size + Clear button (calls a new admin API later; for Phase 10 just a stub that toggles a future switch).
  - ffmpeg detected / missing badge.
  - Plugins folder path display.
  - Database path display.
  - Phase 10 has no new API surface yet — read what Core already exposes via `/api/health` (extend health to include diagnostic fields if needed).

### Task 3 — Shortcuts

- [ ] `useShortcuts`:
  - F2 = rename selected
  - Delete = recycle selected
  - Shift+Delete = permanent delete
  - Ctrl+F = focus search
  - Esc = clear selection / close dialog
  - Arrow keys = navigate grid (already partial via existing keyboard nav).
- [ ] Mount in App.tsx; suppress when an input is focused.

### Task 4 — Accessibility pass

- [ ] Run `npm --workspace @mangoo/web run build` and inspect for axe-core warnings (use built-in via vite plugin if present; otherwise manual audit).
- [ ] Ensure all dialogs have `role="dialog"`, `aria-modal`, `aria-labelledby`, focus trap, focus return.
- [ ] All buttons have `aria-label` when icon-only.
- [ ] Color contrast against the dark stage meets WCAG AA at minimum.

### Task 5 — Frameless chrome (desktop)

- [ ] Electron BrowserWindow `frame: false` + custom titlebar in App.tsx (drag-region on `.topbar`).
- [ ] Window controls (minimize / maximize / close) wired to ipc bridge in `apps/desktop/src/preload.ts`.

### Task 6 — Regression pass

- [ ] `npm test` clean.
- [ ] Manual smoke list (documented in `release-checklist.md`):
  - Add a 1k-file root; scan completes; grid shows; preview switches without lag.
  - Rename / Move / Recycle; recent ops shows them.
  - Tag / rate / favorite; search composes filters.
  - Cancel a running scan; retry it.
  - Disable / re-enable a plugin (registered manifest only — no runtime).
  - Toggle frameless chrome window controls.

### Task 7 — Docs + memory

- [ ] Update `CLAUDE.md` "Current Implementation State" with the final phase-by-phase summary.
- [ ] Update `.codex/memory.md` mirror.
- [ ] Update `docs/superpowers/plans/2026-05-16-mangoo-complete-product-plan.md` Tasks 1–10 checkboxes to ✅.
- [ ] Add `docs/release-checklist.md` with the manual smoke list and release steps (build artifacts, version bump, tag, GitHub release).

### Task 8 — Final commit

- [ ] `npm test` clean.
- [ ] `npm --workspace @mangoo/web run build` clean.
- [ ] `npm --workspace @mangoo/desktop run build` clean.
- [ ] Commit `feat: release hardening (phase 10)`.

---

## Acceptance

- A new user clones the repo, runs `npm install && npm run dev`, sees onboarding, adds a root, browses, organizes, file-ops, plugins page renders even with no plugins, settings page renders. All without dev errors.
- `release-checklist.md` is followable end-to-end by a non-author.
- All ten phases in the master plan show ✅.
