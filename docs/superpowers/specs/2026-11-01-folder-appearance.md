# Folder appearance

## 1. Scope

Ordinary folders in the active Library can choose a Fluent identity symbol and custom color through the shared anchored appearance popup. This extends the Library chooser requested on 2026-11-01. The subsequent reported top-level menu gap is covered by [directory preferences](2026-11-01-directory-preferences.md): RootRecord now has its own appearance endpoint and fields, including pre-scan/offline roots, while provider/reachability state remains visible. A root sentinel is not accepted through this ordinary-folder endpoint.

## 2. Contract and ownership

`PATCH /folders/{folderId}/appearance` accepts the complete icon/color pair and returns authoritative FolderRecord. The request uses the current session and active context revision through `@mangoo/core-client`. The icon is a bounded stable key; unknown well-formed keys remain stored for forward compatibility. New custom colors accept #RGB or #RRGGBB. Null storage fields represent older uncustomized folders; explicit reset writes `folder/#ffffff`.

The catalog database owns the nullable metadata columns through numbered migration 0038. The single writer updates both fields together. No operating-system folder metadata or media files are written. Appearance is not a sync-provider capability or a renderer filesystem operation.

## 3. UI and state

The existing folder context menu opens the shared registered popup. Use ordinary Fluent symbols for folders and the distinct layered Library artwork for Library identities. Both use searchable categories, preset swatches, custom color selection and validated HEX input. Preview updates a local draft; Apply performs one paired write. Cancel/Escape discard it; reset changes the draft. Errors retain the chosen draft, and pending/native-color-dialog dismissal locks preserve the registered overlay family.

Apply the authoritative result to folder child, descendant, ancestor and selected-folder caches. Do not refresh media or reset selection, scroll or layout. Fence older folder reads and stale mutation completions across Library/context switches. Sidebar identities, subfolder fallback glyphs and folder preview identity must agree. Existing image covers keep their precedence.

## 4. Lifecycle and errors

Scanning and in-app rename/move preserve the stable folder record and metadata. Reopening the Library reloads persisted values. Unknown glyphs fall back only when rendered; editing the color must not rewrite an untouched unknown key. Missing, inactive or root-sentinel folder targets are unavailable through this ordinary-folder endpoint. Invalid/missing/extra request fields must be rejected without changing the prior pair.

## 5. Verification

Check OpenAPI/client/schema alignment and typed request transport. Core tests exercise pair atomicity, default/null state, malformed input, unavailable/root targets, context revision, scan/rename and reopen persistence. Mounted UI checks exercise drafts, validation, pending/error retry, reset, metadata cache updates and stale context handling. Actual isolated Electron/Core checks must confirm visible glyphs plus saved metadata after reopen, not merely a successful click.

## 6. Boundaries

Preserve Library CORS PATCH admission, authentication, existing registry-only Library identity refresh, root/provider lifecycle and independent status indicators, media geometry and the global one-boundary pointer-highlight design. A folder icon update must not become an active Library switch or broad media invalidation.

## 7. Examples

Correct: edit an ordinary folder, preview `camera/#a5b6c7`, Apply once, then see the same metadata in the sidebar and folder inspector after reopening.

Wrong: show a larger Library icon grid while claiming each folder can retain its own appearance, or save only renderer-local preferences that disappear in another view.
