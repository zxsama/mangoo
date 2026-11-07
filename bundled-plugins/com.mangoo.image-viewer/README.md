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

Folder browsing with natural / date / size / type sort and wrap-around, prev/next
neighbour prefetch, fit / fill / 1:1 / zoom stops / wheel and pinch zoom at the
cursor, drag pan, rotate and flip, fullscreen with auto-hiding chrome, slideshow,
virtualized filmstrip, info panel with EXIF summary and RGB histogram, background
choices (follow interface, dark, black, checkerboard, light), pixel view above 200%,
animated GIF / WebP / AVIF playback, copy image / path, reveal in folder, open with
the system default app, rename (F2), move to recycle bin (Delete), drag and drop,
always-on-top, window state persistence. Formats are whatever Core decodes
(`crates/core/src/decode/formats.rs` plus running decoder plugins).

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
