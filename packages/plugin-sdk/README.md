# Mangoo Plugin UI SDK 0.1

Build with `node packages/plugin-sdk/build.mjs` from the Host checkout, then `npm pack ./packages/plugin-sdk`. Distribute the resulting versioned tarball to plugin authors. A plugin consumes that artifact; it never imports Host source paths.

Await `pluginBridge().ready` before rendering. The returned context contains the current plugin/view identity and optional Library/root identity. `createCoreClient()` exposes the explicit v1 Core method set over the sandbox message channel. `host` exposes approved folder, clipboard, external-link and view actions. Session tokens, raw URLs, Electron preload and HTTP transport are never exposed. Calls are limited to 32 pending requests with timeouts, abort cleanup, source/session correlation and structured Core errors. Dispose the bridge when ending the view.

`context.rootId` identifies an ordinary Core browsing root and defaults to `null` when older Hosts omit it. `rootUuid` remains available for native plugin-view intents. The SDK validates identity shape only; translating a browsing root into feature-specific records belongs to the plugin.

`@mangoo/plugin-sdk/design` and `/design.css` are built from Host's canonical generic glass, dialog and scrollbar sources; the SDK contains no sync workflow or plugin feature composition. Core DTO declarations are built from the canonical client contracts.

## Shared control root

Each plugin iframe has its own React tree and document; it does not inherit the Host's overlay context or pointer controller. Wrap the plugin UI once with the exported design roots, import the shared stylesheet, and install the shared scrollbar hook once:

```tsx
import type { ReactNode } from "react";
import {
  LiquidGlassLayer,
  OverlayProvider,
  useOverlayScrollbars,
} from "@mangoo/plugin-sdk/design";
import "@mangoo/plugin-sdk/design.css";

export function PluginDesignRoot({ children }: { children: ReactNode }) {
  useOverlayScrollbars();
  return (
    <OverlayProvider>
      <LiquidGlassLayer>{children}</LiquidGlassLayer>
    </OverlayProvider>
  );
}
```

This root is required for shared controls that open layers, including the custom `OverlaySelect`. Its trigger and options use one connected rounded surface and the same global pointer highlight as the Host. Use one provider for the whole plugin UI, not one per field: modal children, outside dismissal and sibling menus must share the same stack. Keep this wrapper inside the plugin's own bridge-ready rendering lifecycle.
