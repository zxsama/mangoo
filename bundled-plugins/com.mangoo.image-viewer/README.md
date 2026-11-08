# com.mangoo.image-viewer — Mangoo 看图

Built-in **Host-engine** plugin: a standalone image viewer that opens the files the
operating system hands to Mangoo, in its own window, with the same design system
and the same image format support as the Library.

Turning the plugin on in Plugin Manager (enable + approve its two permissions) turns
the feature on. Turning it off closes the viewer window, removes the OS
registration and hides "Open in Mangoo 看图" from the Library. Nothing in the
viewer works while it is disabled: Core's `/api/image-viewer/*` routes answer
`403 image_viewer_disabled`.

## Why a Host-engine plugin

Process plugins (`sync-provider.v1`, `file-decoder.v1`) are native executables
behind framed RPC, with UI limited to a sandboxed iframe that cannot load images.
A viewer needs a native OS window, OS file associations and the Host's decode
pipeline, so it is implemented *by the Host*, from this directory:

| Part | Location | Built by |
| --- | --- | --- |
| Manifest (`engine: "host"`, `host.capabilities: ["image-viewer.v1"]`) | `plugin.template.json` | `tools/build/build-bundled-plugins.mjs` stages exactly `plugin.json`; Core pins its SHA-256 in compiled trust with a zero executable digest |
| Desktop integration (window, IPC, argv / second-instance / macOS `open-file`, associations) | `src/desktop/` | `tsc -p tsconfig.desktop.json` → `dist/desktop`, imported by Electron main as `@mangoo/plugin-image-viewer/desktop` |
| Viewer UI and plugin-manager panel | `src/web/` | compiled into the Host web bundle (`@mangoo/plugin-image-viewer/web`), importing only `@mangoo/web/plugin-host` |
| Shared IPC contract, navigation and zoom math | `src/shared/` | both of the above |
| Core services (path-scoped image, folder, thumbnail, metadata/EXIF) | `crates/core/src/api/image_viewer.rs`, `crates/core/src/decode/exif.rs` | Core, gated by `HostRegistry::host_capability_enabled("image-viewer.v1")` |

## Permissions

- `{ "kind": "local-files", "access": "opened-folders" }` — read the images the user
  opens and the other images in their folder. Desktop only acts on (trash, rename,
  reveal, open externally) images directly inside folders the user opened in this
  viewer session.
- `{ "kind": "file-association", "scope": "current-user" }` — register Mangoo as an
  image handler for the current user (Windows HKCU, Linux XDG). Never machine-wide.

## Features

- **Browse**: whole folder, natural / date / size / type sort, wrap-around, neighbour prefetch, virtualized filmstrip, auto-refresh when files appear.
- **View**: fit / fill / 1:1, zoom stops, wheel switches pictures by default (Ctrl+wheel zooms; wheel pans when zoomed in), pinch, drag pan, rotate / flip, pixel view above 200%, five backgrounds, fullscreen, slideshow with progress line.
- **Animated images**: GIF, APNG (`.apng` or `.png`), animated WebP / AVIF play natively; the floating 动图帧 panel (film-reel button, `.` `,` Space) takes over playback with WebCodecs `ImageDecoder` for pause, frame stepping, scrubbing, speed and loop. Core sniffs animation (`x-mangoo-animated`) and never tiers an animated file.
- **Floating panels**: image info (file facts, EXIF, histogram, frame count) and frames float over the picture, drag by the header, snap magnetically to window edges / corners, stay docked through resizes, remember their place (double-click the header to reset).
- **Titlebar**: auto-hides by default and slides in at the top edge; the pin button (or P) keeps it visible.
- **Shortcuts**: every action is remappable in 设置 → 快捷键 (record a chord, conflicts move with an undo, per-action and global reset).
- **Files**: copy image / path, reveal, open with another app, rename (keeps the extension), recycle bin with optional confirmation, drag and drop, always-on-top. Formats are whatever Core decodes.

## OS registration

- **Windows**: one `reg import` of a per-user ProgID (`Mangoo.Image`), OpenWithProgids,
  `Applications\<exe>\SupportedTypes` and Capabilities/RegisteredApplications. Windows
  10/11 protect the actual default, so the viewer deep-links to Settings > Default apps
  for the final choice. Unregistering removes exactly the recorded registration.
- **Linux**: `~/.local/share/applications/mangoo-image-viewer.desktop` plus
  `[Default Applications]` entries in `~/.config/mimeapps.list` (only ours are removed).
- **macOS**: document types come from the app bundle's Info.plist at build time; the
  viewer handles `open-file`, and the status explains Finder's "Open with → Change All".
- Development builds do not write associations unless
  `MANGOO_IMAGE_VIEWER_DEV_ASSOCIATION=1` (an unpackaged Electron needs the dev server).

## Checks

`npm run check:plugin-image-viewer` (typecheck both halves + unit tests). The plugin is
part of the default bundled inventory (`DEFAULT_PLUGIN_IDS` in
`tools/build/plugin-artifacts.mjs`).
