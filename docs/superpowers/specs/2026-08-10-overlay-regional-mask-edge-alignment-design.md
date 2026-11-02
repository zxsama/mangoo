# Overlay Regional Mask Edge Alignment Design

**Status:** Implemented; mounted-graph, real 100% left/right containment, legacy-mutation oracle, and full 100%/125%/150% desktop verification green on 2026-08-10

## Problem

At 100% display scale, the regional blur behind real titlebar popups extends beyond the popup frame by one physical pixel. The defect is visible on both real anchored paths: the left Library switcher and the right Notifications popover. It is not caused by display scaling, fractional placement, the surface gradient, or the shared one-element border.

The regional mask starts with an exact `feFlood` matching the surface's complete outer border box. For rounded surfaces, that flood is blurred and alpha-thresholded to approximate the corner radius. The thresholded result is then consumed directly. A small non-zero alpha tail survives just outside the original flood rectangle, so the filtered application plane paints beyond the portal surface's visible frame.

## Required behavior

- The source mask rectangle remains exactly equal to the popup surface's complete outer border box.
- The source rectangle, width, height, and radius are never inset for the border, padding, or anti-aliasing.
- Rounded-mask construction may use expanded intermediate primitive bounds, but the final alpha support must not extend outside the original source flood.
- The popup's existing elevation shadow remains an intentional, separate outer halo. The regional blur must not be mistaken for or merged with that shadow.
- Radius-zero masks continue to use the exact hard source flood directly.
- Multiple rounded masks are clipped individually before they are merged.
- The graph remains synchronous and contains no `feImage`, snapshot, decode, or asynchronous resource path.

## Filter graph

For each rounded mask, build this graph:

```text
exact source flood
  -> mask-only Gaussian blur with expanded primitive bounds
  -> alpha threshold (rounded, temporarily allowed to have a tail)
  -> feComposite(in=rounded, in2=exact source, operator=in)
  -> clipped rounded mask
```

The final `feComposite` is an alpha-support clip, not a geometry contraction. Its `in2` must be the original exact source result. Downstream inside/outside composites and mask unions consume only the clipped result.

The main regional image path remains unchanged:

```text
SourceGraphic -> Gaussian blur -> saturation
saturated IN clipped-mask + SourceGraphic OUT clipped-mask -> merged output
```

## Frame contract

The overlay surface keeps the approved historical material contract:

- the 108-degree gradient and `1px solid rgba(255, 255, 255, 0.3)` border are painted by the same `.overlay-surface` element;
- the generic `.liquid-glass::after` plane does not paint a second overlay border or inset edge shadow;
- `background-clip` remains the historical border-box behavior;
- the regional mask continues to use the surface's unmodified outer border box;
- no test or production path may compensate by moving, resizing, or padding the popup.

## Verification contract

### Mounted graph

The mounted lifecycle fixture must prove all of the following from the live SVG DOM:

- the source flood equals the visible surface outer border box;
- the mask Gaussian still has expanded bounds;
- a case with at least two rounded masks records every mask chain independently;
- each rounded alpha-threshold result is followed by exactly one source clip;
- each source clip uses `operator="in"`, its own rounded result as `in`, and its own original flood result as `in2`;
- the mask union consumes only those independently clipped results and never a source, softened, or unclipped rounded result;
- a radius-zero mask bypasses rounding and contributes its exact hard source flood directly;
- the downstream regional composites use the clipped result or the union of clipped/hard-source results;
- radius-zero, fallback, cleanup, nested, modal, and atomic stack behavior remain unchanged.

### Real titlebar popups at 100%

The desktop visual regression must exercise the real application paths without geometry overrides:

- left: `[data-titlebar-control="library-switcher"]` -> `#library-root`;
- right: `[data-titlebar-control="notifications"]` -> `#notifications-root`.

For each popup, set device scale to exactly `1`, keep page scale at `1`, and insert a pointer-inert, deterministic high-frequency diagnostic layer inside the real `#root` beneath the body portal. The layer must cover all four popup edges while leaving the real popup's natural position and size untouched. Wait for `data-overlay-state="open"`, `transform:none`, stable geometry, and a settled regional graph, then keep that same popup mounted while capturing `zero-before`, `default`, and `zero-after`; only the regional dialog-blur value may change between the three frames.

The surface rect, root rect, border/background/shadow computed styles, `data-overlay-state`, and transform must be identical in all three snapshots. The expected outer box is computed independently from the live surface and root `getBoundingClientRect()` values; it is not copied from filter attributes or planner output.

For each straight edge, the analyzer uses the literal RGB classification threshold `3` channel levels:

- in the 2–5 physical-pixel band inside the frame, the default-versus-zero mean absolute RGB difference must be at least `4.0`, and at least `60%` of sampled pixels must exceed the classification threshold;
- in the first physical pixel outside the frame, the number of default-versus-zero classified pixels must be exactly `0`;
- the same inside/outside bands in `zero-before` versus `zero-after` must have mean absolute RGB difference at most `0.5` and exactly `0` classified pixels.

These positive inside requirements prevent an empty or flat-background pass. The zero-before/zero-after control proves that the retained real elevation shadow and temporal noise are below the classifier and are not being attributed to the regional delta. Spatial tolerance remains `0` physical pixels.

The pixel oracle must also prove that it detects this exact defect. While each real popup remains mounted at the same geometry, a controlled test-only mutation temporarily rewires downstream mask consumers from the final clipped result to that clip's unclipped rounded input, reproducing the legacy graph without changing the surface, diagnostic layer, blur value, or shadow. In this legacy-unclipped frame, each of the left and right popup cases must report at least one straight edge with one or more outside pixels above the RGB threshold. After restoring the clipped graph, all four outside edge counts must return to `0`. A harness that cannot distinguish those two graph topologies fails even if its normal capture is green.

The test must also record that the source flood still equals the independently measured outer border box. It must not set popup `left`, `top`, `width`, `height`, or `transform`, and it must not hide the frame while measuring containment.

The 125% and 150% matrix remains required for the broader blur/stack contract, but the edge-containment defect must be independently covered and pass at 100%.

## Supersession

This design narrows and corrects the frame-alignment evidence in `2026-08-09-overlay-frosted-material-and-radius-unification-design.md` and Task 4 of `2026-08-10-overlay-in-app-acrylic-sampling-fix.md`. Their static same-element outline probe remains valid for border ownership, but it does not prove containment of the separately rendered regional filter plane.

## Implementation evidence

- The mounted lifecycle suite records each rounded threshold, its own `feComposite operator="in"` source clip, union inputs, and radius-zero hard-flood bypass; all `12/12` tests pass.
- Fresh OS-backdrop run `mangoo-visual-msmfm6a1-66048` passed with `hardFailures: []`. At 100%, Library-left and Notifications-right normal outside counts are `[0,0,0,0]` in left/right/top/bottom order, their zero controls are identically zero, and their restored-clipped counts are `[0,0,0,0]`.
- The same mounted-popup mutation oracle detected the legacy graph on every edge: Library-left `[155,155,427,427]`; Notifications-right `[151,151,327,327]`. Restoring the clipped graph returned all four counts to zero without changing geometry or material state.
- The source floods remained equal to the independently measured outer boxes: Library-left `12.03125,44.328125,460 x 188`; Notifications-right `955.96875,44.671875,360 x 184` (capture metadata rounds the independent expected coordinates to three decimals).
- The broader popup/modal matrix passed at device scales `1`, `1.25`, and `1.5` with page scale `1`. The retained real-content gate covered six real images and three DOM text rectangles; image boundary energy was `40.3865 -> 0.3793 -> 0.0887`, text edge energy was `19.2672 -> 0.3324 -> 0.1271`, and both outside-reference deltas were `0`.
