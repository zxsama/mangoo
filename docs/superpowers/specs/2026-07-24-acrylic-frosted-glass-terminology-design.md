# Megle Acrylic Frosted Glass Terminology Design

Updated: 2026-07-24

## 1. Purpose

Megle's current documentation positions the interface as a "Liquid Glass" design. That label is now too closely tied to an Apple-specific visual direction and no longer describes Megle's Windows-first desktop material accurately.

This change updates the current design positioning to a layered acrylic frosted-glass language without changing the implemented UI, code identifiers, file paths, tests, or runtime behavior.

## 2. Approved Positioning

The canonical first-use term is:

`分层亚克力毛玻璃设计（Layered Acrylic / Frosted Glass）`

After the first definition in a document, prose should use the most precise short form for the context:

- `亚克力材质` for the Windows desktop shell, backdrop, chrome, panels, and overlays.
- `毛玻璃层` for translucent surfaces whose defining property is background blur.
- `分层玻璃材质系统` when describing the complete hierarchy of chrome, controls, overlays, and stable content surfaces.

The positioning remains Windows-first, restrained, and content-focused. Acrylic and frosted-glass materials belong to chrome, controls, floating surfaces, and overlays. Media grids, previews, and other content-heavy surfaces remain sharp and stable.

## 3. Documentation Scope

Update current authoritative descriptions in:

- `AGENTS.md`
- `CLAUDE.md`
- `docs/README.md`
- `docs/implementation-roadmap.md`
- `docs/open-questions.md`
- `docs/component-library-review.md`
- `docs/final-solution.md`
- `docs/superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md`

The existing canonical specification filename is preserved so links remain stable. Its title and natural-language positioning will use the approved acrylic frosted-glass terminology.

Historical plans, completed feature specifications, verification reports, and archived decision records remain unchanged. They record the terminology used when those decisions were made and are not current product-positioning surfaces.

## 4. Protected Technical Terms

Do not rename or rewrite technical identifiers and literal references, including:

- `LiquidGlassSurface`
- `LiquidGlassButton`
- `design/liquid-glass`
- `liquid-glass` CSS classes, data attributes, and token names
- existing specification and plan filenames
- validator names, test expectations, commands, and code snippets
- proper-noun references such as Apple's `Liquid Glass` documentation or third-party package names

If a sentence contains a protected identifier, update only the surrounding descriptive prose.

## 5. Non-Goals

- No TypeScript, TSX, CSS, Rust, JavaScript, JSON, or configuration changes.
- No component, directory, class, token, test, or validator renames.
- No visual redesign or material-parameter changes.
- No reinterpretation of historical implementation decisions.
- No broad rewrite of every Markdown occurrence.

## 6. Verification

The documentation update is complete when:

1. `git diff --name-only` contains only the approved documentation files.
2. `git diff --check` reports no whitespace errors.
3. Current positioning text uses the approved acrylic frosted-glass terminology.
4. Every remaining `Liquid Glass` occurrence in changed files is either a protected identifier, stable path or filename, proper noun, or historical reference that must remain literal.
5. No runtime or source-code verification is required because the implementation is unchanged.
