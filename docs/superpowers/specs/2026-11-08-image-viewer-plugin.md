# Built-in image viewer plugin (com.mangoo.image-viewer)

## Goal

Ship a cross-platform image viewer that behaves like the mainstream standalone
viewers (Windows Photos, IrfanView, XnView, FastStone, ImageGlass, Honeyview, macOS
Preview, GNOME Loupe): the OS opens a picture with Mangoo, Mangoo shows it in its own
window independent of the Library, and the user can browse the folder. The plugin
lives in its own directory, `bundled-plugins/com.mangoo.image-viewer/`, follows the
Host design language and supports exactly the formats the project decodes. When the
plugin is disabled, every viewer surface is off.

## Survey of mainstream viewers → feature set

| Capability | Typical viewers | Mangoo 看图 |
| --- | --- | --- |
| Open from OS / default app | all | argv, packaged single-instance `second-instance`, macOS `open-file`; per-user registration (Windows HKCU, Linux XDG) |
| Browse folder, wrap-around | all | Core folder listing, natural / date / size / type order, asc/desc, loop toggle, refresh on window focus |
| Fast prev/next | FastStone, Honeyview | worker-decoded `ImageBitmap` LRU (768 MiB) + nearest-first neighbour prefetch, stable canvas |
| Fit / fill / 1:1 / zoom stops | all | fit (optionally enlarge small), fill, actual size (device pixels), stop ladder 2%–3200% |
| Zoom at cursor, pan, pinch | all | wheel (or Ctrl+wheel), pinch, drag pan with edge clamping, double-click fit⇄1:1 |
| Large-image sharpness | XnView | adaptive Core tier for the viewport, original fetched when zoomed past the tier |
| Rotate / flip (view) | all | 90° steps, horizontal / vertical flip, animated transitions |
| Fullscreen, slideshow | all | F11 with auto-hiding chrome, slideshow 2–20 s |
| Filmstrip | Photos, ImageGlass | virtualized horizontal strip, Core WebP thumbnails |
| Info / EXIF / histogram | XnView, FastStone | file facts, camera / exposure / lens / GPS summary, RGB histogram |
| Background / transparency | ImageGlass, Honeyview | follow interface, dark, black, checkerboard, light |
| Pixel view | ImageGlass | nearest-neighbour above 200% (toggle) |
| Animated images | all | GIF / animated WebP / AVIF sequences on an animating `<img>` |
| File actions | all | copy image / path, reveal, open with default app, rename (F2), recycle (Delete, confirm optional), drag & drop open |
| Window | all | frameless acrylic window, state restore, always-on-top |

Deliberately out of scope: lossy re-save of rotations (view-only, like Photos'
non-destructive rotate), editing / annotation, printing, wallpaper, permanent delete.

## Plugin-system extension: Host engine

Process plugins are trusted native executables behind framed RPC, and plugin-owned UI is
a sandboxed iframe with `img-src data:` / `connect-src 'none'`. Neither can provide an
OS window, file associations or the decode pipeline, so the manifest gains a second
engine:

- `engine: "host"` + `host: { capabilities: ["image-viewer.v1"] }`. Host manifests omit
  `entry`, `rpc`, `integrity` and `contributions` (rejected even when empty), may only use
  activation events `file-open-requested` / `diagnostics-requested`, and may only request
  the new Host permissions `local-files { access: opened-folders }` and
  `file-association { scope: current-user }`. Process manifests cannot declare any of these.
- Artifact = exactly `plugin.json`. Build tooling stages it with inventory row
  `{ id, version, platform, engine: "host", manifestSha256 }`; compiled trust pins the
  manifest digest with a zero executable digest. `verify_bundled_host_plugin` requires that
  marker, and `verify_bundled_entry` refuses host manifests, so neither kind can vouch for
  the other.
- Lifecycle reuses enable / grants / disable. Host plugins create no account row and never
  enter Provider or decoder launch enumeration. A launchable intent settles straight to
  `ready` with the declared capabilities in `capabilities_json`; any other state publishes
  none and leaves no pending stop (`settle_host_engine_runtime`).
- `HostRegistry::host_capability_enabled(capability)` is the single gate for Host services.
- Web: `features/plugins/hostPlugins.tsx` renders a Host plugin's panel in Plugin Manager,
  provides `useHostCapability`, and readable permission labels. Plugin web code imports Host
  primitives only through `apps/web/src/plugin-host` (`@mangoo/web/plugin-host`).
- Desktop: `broadcastPluginChange()` also invokes `mangoo:plugins-changed`, so Desktop-side
  Host plugins reconcile after any lifecycle mutation without polling.

## Core services (`/api/image-viewer/*`)

`formats`, `folder`, `image`, `thumbnail`, `metadata`. They are path-scoped because OS-opened
files are usually outside every Library, and every route answers `403 image_viewer_disabled`
unless the capability is enabled. They are exempt from the active-Library requirement and
write nothing (no Library rows, sidecars or `preview_blobs`).

The path policy accepts only absolute, `..`-free, NUL-free, bounded paths to regular files
whose extension is in `decode/formats.rs` or a running decoder plugin's declaration.
`image` reuses `resolve_preview_plan` and `generate_preview_webp`, so tiers, transcodes and
degrade-to-original behave as in the Library preview. Plugin formats decode through
`DecoderManager::decode_owned`. Generated WebP lives in a process-wide in-memory LRU
(384 MiB images, 64 MiB thumbnails) behind separate `spawn_blocking` semaphores, so
filmstrip work never starves the visible picture. `metadata` adds a bounded dependency-free
EXIF reader (JPEG APP1, PNG eXIf, WebP EXIF, TIFF/RAW, HEIF/AVIF `Exif` item).

## Desktop

`createImageViewerHost(electron, options)` owns:
- one reusable frameless acrylic viewer window (role `image-viewer`, common window-chrome IPC
  only, no notification access);
- the plugin's IPC, each channel authorized for the viewer window, and only the association
  and open-files channels for privileged Host windows;
- argv / second-instance / `open-file` handling;
- association lifecycle;
- window state in `userData/image-viewer.json`.

A launch that only opens images shows just the viewer. Any other launch, or any launch while
the plugin is disabled, opens the Library. File actions run only on images directly inside
folders the user opened in this viewer session (`ViewerAccessPolicy`).

Disabling the plugin closes the window and removes the recorded registration. The user's
"wanted" choice survives, so re-enabling re-registers it.

## Verification

- Rust: manifest strictness, host trust marker, registry lifecycle gate, path policy, LRU,
  EXIF parsing.
- JS: schema fixtures, artifact staging / trust rendering, plugin unit tests (argv, access
  policy, state, Windows .reg and XDG generation / removal, navigation, zoom math, animation
  detection, preload channel parity), Host desktop / web / UI-design / structure gates.
- Real Electron run: enable plugin → open images → navigate / zoom / info / filmstrip;
  disable → `403` and window closed.
