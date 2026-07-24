# Acrylic Frosted Glass Terminology Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the outdated Liquid Glass product-positioning prose in Megle's current authoritative documentation with the approved layered acrylic frosted-glass terminology.

**Architecture:** Make documentation-only edits in the eight approved authority files. Preserve source-code identifiers, paths, filenames, validator/test literals, Apple proper nouns, third-party package names, and historical plans or reports.

**Tech Stack:** Markdown, Git, ripgrep

---

### Task 1: Update current UI material positioning

**Files:**
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `docs/README.md`
- Modify: `docs/implementation-roadmap.md`
- Modify: `docs/open-questions.md`
- Modify: `docs/component-library-review.md`
- Modify: `docs/final-solution.md`
- Modify: `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md`

- [x] **Step 1: Replace current positioning prose**

Use `分层亚克力毛玻璃设计（Layered Acrylic / Frosted Glass）` at the canonical definition, then use `亚克力材质`, `毛玻璃层`, or `分层玻璃材质系统` according to context.

- [x] **Step 2: Preserve protected literals**

Keep `LiquidGlassSurface`, `LiquidGlassButton`, `design/liquid-glass`, existing filenames, CSS/data/token names, test and validator literals, Apple's `Liquid Glass` title, and third-party package names unchanged.

- [x] **Step 3: Verify the documentation-only scope**

Run:

```powershell
git diff --name-only
git diff --check
rg -n -i "Liquid Glass|LiquidGlass|liquid-glass|liquid glass|液态玻璃" AGENTS.md CLAUDE.md docs/README.md docs/implementation-roadmap.md docs/open-questions.md docs/component-library-review.md docs/final-solution.md docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md
```

Expected: only approved Markdown files plus this plan are changed; whitespace check passes; remaining matches are protected literals or proper nouns.

- [x] **Step 4: Commit the documentation update**

```powershell
git add AGENTS.md CLAUDE.md docs/README.md docs/implementation-roadmap.md docs/open-questions.md docs/component-library-review.md docs/final-solution.md docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md docs/superpowers/plans/2026-07-24-acrylic-frosted-glass-terminology.md
git commit -m "docs: update acrylic glass positioning"
```
