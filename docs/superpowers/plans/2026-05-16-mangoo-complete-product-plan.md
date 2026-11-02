# Mangoo Complete Product Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the approved UI foundation into the full Mangoo product roadmap and drive implementation through a complete Windows-first product, not just a shell or prototype.

**Architecture:** Keep the existing `Electron shell -> React UI -> Rust Core API -> SQLite/media pipeline` structure. Treat UI foundation as a prerequisite layer, then complete the browse path, preview path, task system, metadata/search, file operations, advanced media, plugins, Web/Docker reuse, and release hardening in sequence.

**Tech Stack:** Electron, React, TypeScript, Vite, Radix UI, Tailwind CSS, Lucide, TanStack Query, Zustand, Rust, Axum, SQLite, FFmpeg, libvips, process plugins.

---

## File Structure

**Existing Inputs:**

- `D:/Mangoo/docs/implementation-roadmap.md`
- `D:/Mangoo/docs/superpowers/specs/2026-05-16-mangoo-ui-liquid-glass-design.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-ui-foundation.md`
- `D:/Mangoo/docs/final-solution.md`
- `D:/Mangoo/.codex/memory.md`

**Create Over Time:**

- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-real-directory-browsing.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-thumbnail-preview.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-task-center-and-watcher.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-metadata-search.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-file-operations.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-advanced-media.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-plugin-manager.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-web-docker.md`
- `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-release-hardening.md`

**Modify During Coordination:**

- `D:/Mangoo/docs/implementation-roadmap.md`
- `D:/Mangoo/docs/README.md`
- `D:/Mangoo/.codex/memory.md`

**Verify With:**

- `npm test`
- `npm --workspace @mangoo/web run build`
- `npm --workspace @mangoo/desktop run build`
- phase-specific benchmark or manual acceptance steps from each child plan

### Task 1: Execute The UI Foundation First

**Files:**

- Modify: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-ui-foundation.md`
- Modify: `D:/Mangoo/.codex/memory.md`
- Verify: `npm test`

- [x] **Step 1: Treat the UI foundation plan as the first implementation gate**

Use `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-ui-foundation.md` as the first executable slice.

- [x] **Step 2: Do not start feature-specific page polish before the shell is real**

Required UI baseline before Phase 2:

- frameless Electron desktop chrome
- shared `design-tokens` package
- shared `ui` package
- reusable `app-shell`
- Library / Settings / Plugins / Tasks page scaffolding

- [x] **Step 3: Verify the foundation slice**

Run:

```bash
npm test
npm --workspace @mangoo/web run build
npm --workspace @mangoo/desktop run build
```

Expected:

- foundation changes compile cleanly
- the repo checks still pass
- later phases can build on one design system instead of per-page CSS

### Task 2: Turn Phase 2 Into The First Real Product Slice

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-real-directory-browsing.md`
- Modify: `D:/Mangoo/docs/implementation-roadmap.md`
- Verify: `npm test`

- [x] **Step 1: Write the detailed Phase 2 browsing plan**

Scope the child plan to:

- add root
- scan root
- populate SQLite
- show real folder tree
- show media grid
- open preview entry point
- use the shared toolbar / sidebar / inspector / context menu shell

- [x] **Step 2: Keep UI and Core work coupled**

The Phase 2 child plan must cover both:

- Core scan/query endpoints
- UI workbench integration inside the shared shell

- [x] **Step 3: Verify the first product slice**

Acceptance target:

- a user can add a real root and browse a real directory in the designed UI

### Task 3: Integrate Thumbnail And Preview Quality Into The Product

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-thumbnail-preview.md`
- Verify: performance benchmark references under `D:/Mangoo/docs/performance-results/`

- [x] **Step 1: Write the thumbnail/preview child plan**

Scope it to:

- tiny/grid/preview pipeline
- viewport-priority scheduling
- tile loading states
- preview transition behavior
- neighbor prefetch

- [x] **Step 2: Keep visual behavior inside the approved UI rules**

The child plan must explicitly keep:

- glass on control layers
- stable dark content surfaces
- no layout shift on tile load
- no expensive blur over the grid

- [x] **Step 3: Verify against the existing performance gates**

Acceptance target:

- cached preview visible quickly
- left/right switching remains responsive
- no grid performance regression from glass UI

### Task 4: Unify Background Tasks, Watchers, And Recovery

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-task-center-and-watcher.md`
- Modify: `D:/Mangoo/.codex/memory.md`

- [x] **Step 1: Write the watcher/task-center child plan**

Scope it to:

- background scan queue
- watcher overflow handling
- task drawer
- task center page
- retry / failure UI

- [x] **Step 2: Treat this as both backend and product work**

This phase is not only queue design. It must also deliver:

- visible task progress
- traceable failures
- consistent recovery UI

- [x] **Step 3: Verify product-level behavior**

Acceptance target:

- external file changes appear in Mangoo
- long-running work is visible and recoverable in the UI

### Task 5: Complete Metadata, Search, And Organizing Workflows

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-metadata-search.md`
- Verify: `npm test`

- [x] **Step 1: Write the metadata/search child plan**

Scope it to:

- tags
- ratings
- favorites
- notes
- search field
- filter chips
- sort menu
- saved views

- [x] **Step 2: Keep organization features inside the shell language**

The child plan must cover:

- toolbar search
- filter surfaces
- inspector metadata editing
- no split between “pretty shell” and “plain settings-like forms”

- [x] **Step 3: Verify the organizing loop**

Acceptance target:

- a user can browse, search, tag, rate, and refilter the same library without leaving the product flow

### Task 6: Finish Real File Operations As Product Features

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-file-operations.md`
- Verify: file operation consistency checks and manual confirmation flows

- [x] **Step 1: Write the file-operations child plan**

Scope it to:

- rename
- move
- recycle-bin delete
- conflict handling
- operation logs
- UI recovery flows

- [x] **Step 2: Make dangerous actions part of the UI system**

The child plan must include:

- context menus
- confirmation dialogs
- progress
- partial-failure reporting
- task history

- [x] **Step 3: Verify the real-product bar**

Acceptance target:

- the user can trust Mangoo to operate on real files with visible, reversible, and well-explained results

### Task 7: Add Advanced Media Support Without Breaking The Main Product

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-advanced-media.md`

- [x] **Step 1: Write the advanced-media child plan**

Scope it to:

- FFmpeg metadata
- poster frames
- video preview
- long-tail image decoder path

- [x] **Step 2: Preserve the same product interaction model**

The child plan must explain how video and advanced formats still fit:

- the same grid
- the same preview shell
- the same inspector
- the same task and failure model

- [x] **Step 3: Verify graceful degradation**

Acceptance target:

- unsupported or slow formats still fail gracefully inside the same product UX

### Task 8: Make Plugins A First-Class Product Area

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-plugin-manager.md`

- [x] **Step 1: Write the plugin-manager child plan**

Scope it to:

- plugin manifest handling
- enable/disable
- permissions
- logs
- plugin settings
- plugin manager page

- [x] **Step 2: Keep plugins inside the approved shell**

The child plan must deliver:

- plugin page in the main app shell
- plugin detail inspector
- error isolation and visibility

- [x] **Step 3: Verify the extensibility loop**

Acceptance target:

- plugins feel like part of the product, not a bolted-on developer tool

### Task 9: Reuse The Product For Web/Docker

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-web-docker.md`

- [x] **Step 1: Write the Web/Docker child plan**

Scope it to:

- headless Core mode
- auth
- mounted roots
- HTTP asset delivery
- UI reuse

- [x] **Step 2: Treat this as adaptation, not redesign**

The child plan must preserve:

- the same React app shell
- the same page structure
- the same component system

- [x] **Step 3: Verify product continuity**

Acceptance target:

- the deployment model changes, but Mangoo still feels like the same product

### Task 10: Close The Loop With Product Hardening And Release

**Files:**

- Create: `D:/Mangoo/docs/superpowers/plans/2026-05-16-mangoo-release-hardening.md`
- Modify: `D:/Mangoo/docs/README.md`
- Modify: `D:/Mangoo/.codex/memory.md`

- [x] **Step 1: Write the release-hardening child plan**

Scope it to:

- settings polish
- onboarding / empty states
- shortcut pass
- accessibility pass
- regression pass
- release checklist

- [x] **Step 2: Define full-product done**

The release-hardening plan must define “complete product” as:

- main workflows work
- failure workflows work
- dangerous actions are trustworthy
- design system covers the whole app
- desktop build is demoable and releasable

- [x] **Step 3: Verify and update memory**

Run:

```bash
npm test
npm --workspace @mangoo/web run build
npm --workspace @mangoo/desktop run build
```

Expected:

- the repo still verifies cleanly
- docs and memory point to the integrated full-product plan
