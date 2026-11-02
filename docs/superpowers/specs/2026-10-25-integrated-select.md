# Integrated glass dropdowns

## Scope

Single-value `OverlaySelect` controls use a custom rounded dropdown joined to the trigger. This supersedes the native-menu palette correction: changing native option colors cannot provide the requested shape, material or shared outline. History filters, process-log filters, plugin configuration and the plugin SDK use the same implementation. Existing custom menus retain their shared primitives. Native `multiple` or `size > 1` listboxes remain a separate compatibility branch.

## Public and shared interfaces

```ts
export interface OverlaySelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label: string;
}
```

`OverlayField.tsx` re-exports the component and props to preserve existing imports. Children remain native `option`/`optgroup` elements. The real select owns form state, labels, focus and change-event targets; consumers can continue reading `event.currentTarget.value` or `event.target.value`.

`AnchoredOverlay` supplies `integratedHeader`, `interactivePointerOwner`, `onPresenceChange` and `restoreFocus` options. `OverlayStackApi.getContainingLayer` discovers the registered layer containing the actual trigger. A select inherits that layer's family/parent instead of opening a competing root.

Production application roots already include `OverlayProvider`. Standalone plugin documents must wrap their UI once with `OverlayProvider` and `LiquidGlassLayer`, initialize `useOverlayScrollbars`, and load the distributed design CSS; see `packages/plugin-sdk/README.md`. An iframe cannot inherit its host document's React context.

## Rendering and interaction contract

- The open surface covers the original trigger rectangle. Its header has the trigger's height and width, and the options join that header without a gap or internal outline. Bottom placement puts the header first; top placement puts it last, still aligned to the original trigger.
- Keep the original control painted until the integrated portal is present and its measured layout exists. `AnchoredOverlay.onPresenceChange` reports that readiness in a layout effect; `OverlaySelect` derives coverage from that signal, not from `open`. The first visible header must replace the original control in the same paint, without a blank or duplicate-header frame. Non-integrated overlay presence semantics remain unchanged.
- Placement includes the header in the viewport budget. Only the options viewport scrolls, using the existing overlay scrollbar and its 5px thumb.
- Compact option rows use a 24px minimum height, 18px line height and `3px 8px` padding. The options container uses `2px 4px` padding. Keep the trigger/header height unchanged; reduce the list's vertical whitespace rather than shrinking the text or detaching the header.
- The joined surface is the sole global pointer-highlight owner via `data-interactive-pointer-owner="true"`. The original trigger opts out while covered, including the shared close-presence interval. Rows do not become independent pointer owners.
- Reuse the existing pointer driver and material tokens. Clear the old control/focus outline and inherited ring/shadow. Do not add a per-row border, hover fill, local pointer tracker or animation that separates the header from the surface. Selection uses a fill; keyboard feedback emphasizes text.
- DOM focus stays on the real select while active-descendant navigation tracks options. Native popup opening is suppressed for the custom dropdown. Arrow keys, Home/End and typeahead navigate; commit/cancel, outside dismissal and native Tab order remain available.
- Controlled values remain owner-controlled. Uncontrolled defaults, explicit empty values, form reset, dynamic children, option/optgroup disabled state and actual native change-event targets are preserved.
- Opening a child in the current modal is valid. Its registered z-order stays above the parent, Escape dismisses only the child, and Tab advances within the parent. Closing/removing a parent dismisses its descendants through the shared lifecycle.

## Edge and failure behavior

| Case | Required result |
| --- | --- |
| Empty or all-disabled options | No invalid selection or navigation exception |
| Disabled select, option or optgroup | Disabled values cannot be committed by the custom list |
| Controlled owner declines a change | Rendered/native value returns to the owner's value |
| Native label or direct click without pointerdown | Focus the real trigger before discovering/opening its layer |
| Sibling select inside a modal | Transfer the child popup while preserving the modal |
| Long list near a viewport edge | Flip/clamp within the viewport; header remains on the trigger |
| Parent removed or component unmounted | Remove descendants, listeners and portal state through shared lifecycle |
| Standalone SDK root without provider | Fix the root composition; do not create a per-select private overlay stack |

## Examples

Good: retain the existing consumer contract.

```tsx
<OverlaySelect label="状态" value={status} onChange={(event) => setStatus(event.currentTarget.value)}>
  <option value="">所有状态</option>
  <option value="running">进行中</option>
  <option value="unavailable" disabled>暂不可用</option>
</OverlaySelect>
```

Baseline: a closed field presents one rounded control; opening it extends that same visible outer surface. An above-opening field mirrors the placement without moving the header away from the original control.

Bad: recoloring a native OS dropdown, placing a separate rounded menu below a still-outlined trigger, adding option hover backgrounds/rings, or casting a fabricated event object to a native select event.

## Verification

- `overlayPosition.test.mts` and `overlayStack.test.mts` guard header/viewport geometry, legitimate modal children and descendant lifetime.
- `integratedSelectMounted.test.mts` mounts the real component and shared modal. Its 14 groups cover forms/events, keyboard, disabled/dynamic options, sibling transfer, direct click/labels, scrolling, above/below placement, one pointer owner, focus and cleanup. Real requestAnimationFrame sampling also checks visible-control continuity, no duplicate header, first-visible header alignment and compact spacing across normal/reduced motion, repeat opening, top placement and modal/sibling cases.
- Actual notification History/Log consumers verify selection reaches queries/subscriptions. A built SDK JS/CSS fixture verifies the distributed component, not just source exports.
- Renderer screenshots establish geometry, state, corners, layering and highlight ownership. Actual behind-window compositing is verified separately with Windows compositor capture; renderer screenshots do not establish physical-display anti-aliasing.
- Run Web, Desktop, UI-design and SDK gates for changes to this shared contract. Full task validation and known legacy-harness limits are recorded in the parent acceptance report.

## Regression lesson

Wrong: relying on a low-specificity top-placement rule while a later generic surface rule sets `flex-direction: column`. The popup can report top placement while its header renders at the top of the list, detached from the original trigger.

Correct: make the integrated top-placement rule authoritative and assert the rendered header's top/height against the trigger rectangle after an actual upward flip. Screenshot inspection and the mounted geometry assertion both guard this behavior.

Wrong: covering the original control with `open || present` while the portal is still waiting for a scheduled measurement. Settled screenshots and selection tests can pass despite invisible-control/hidden-popup frames during opening.

Correct: cover it only after integrated presence reports a measured surface, and sample from before the actual opening input through the first visible frames. In every sampled frame the original control or its replacement must be visible; once the popup is visible, the original is covered and the header matches its rectangle.
