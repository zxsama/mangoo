# Library appearance editor and reliable saving

## 1. Scope / trigger

The user reports that Library icons/colors cannot be changed and requests a different interaction design. A production-like owned Desktop run using a Vite HTTP origin reproduced rejected PATCH preflights: Core's CORS method list omitted PATCH. Earlier packaged and Core-metadata checks did not exercise this cross-origin path. A separate static gap exists in appearance invalidation: successful metadata writes must refresh visible titlebar/list/Settings consumers without switching or reloading media.

The latest follow-up keeps one shared editor for the switcher, Settings and creation form, presented as an anchored glass popup rather than a standalone modal. Use a richer Fluent System Icons identity collection and special flat-skeuomorphic Library artwork; general command/navigation glyphs remain Phosphor. Preserve stored icon keys/colors, white default, shared acrylic, whole-row switcher highlighting and one thin boundary per shape. Library appearance continues using its existing metadata endpoint and storage.

## 2. Signatures and ownership

- Existing persistence: `PATCH /api/libraries/{libraryUuid}` with the current session and `X-Mangoo-Context-Revision` headers and the icon/color pair.
- Core CORS must permit PATCH preflight for origins/headers already admitted by its existing policy; preserve authentication, revision checks and other CORS restrictions. Do not solve this with a renderer bypass or broader wildcard policy.
- Shared appearance editor owns its session draft, preview, pending/error state, Apply/Cancel and draft reset. Callers own what applying means.
- Switcher/Settings apply through `setLibraryIcon`; successful persistence publishes the existing registry-only invalidation signal. App/titlebar and other consumers reread authoritative metadata with generation/unmount guards.
- Creation applies only to the parent form draft. The existing create/preflight flow persists the final pair when creating the Library.

The default development entry `start-desktop.cmd` runs `npm run dev`; `tools/dev/run-dev.mjs` builds debug Core on each start unless external-Core mode is explicitly configured. Already-running Core does not hot-reload Rust/CORS changes. Close all old Mangoo application windows and wait for their normal shutdown before restarting the entrypoint, so the running executable is released and the corrected Core can be built/loaded. Frontend refresh alone does not activate this fix. The production launcher rebuilds when its source-input hash changes, and distributed verification must check the actual packaged Core rather than only `target/release`.

## 3. Interaction contract

The compact popup uses a 14px/20px semibold title, a small identity preview and two labelled single-selection groups: a glyph grid and a color-swatch grid. Keep choices large enough for ordinary pointer input, show selection using fill/checks and give colors accessible names. Glyph choices use a stable theme foreground so a dark custom color cannot make every option illegible; the preview and actual Library glyph use the chosen color. Swatch checks must contrast with both white and dark custom values without rewriting the saved color. Each group has one Tab stop, with arrows moving within that group. Use the registered overlay's keyboard owner; global workspace shortcuts must not intercept its controls.

The compact identity grid uses 18px glyphs, 28px-high cells with a minimum width of 28px, and zero grid gap. Responsive columns fit the available width (11 in the standard 344px popup); preserve the scroll viewport's 172px maximum height, allowing short categories to shrink to content. The glyph-to-glyph whitespace is still about 10px, while targets remain at least 28px. These sizes apply only to selectable glyphs, not preview/titlebar/sidebar identities or color controls. Arrow movement follows measured rendered rows, so resizing, filtering and enlarged text cannot leave keyboard navigation using a stale fixed column count. Later choices must scroll into view when focused.

The identity catalog contains 502 keys in 20 searchable types: the existing 15 pictogram/number types plus uppercase letters, lowercase letters, uppercase Roman numerals, lowercase Roman numerals and Chinese numerals. Preserve existing keys, flat catalog order and artwork when extending it; the first 384 identities retain their complete descriptors and paths through the character addition. Category reassignment is local presentation metadata and does not rewrite saved icon keys. Fluent pictograms retain their pinned official source and paired paths; typographic identities use explicitly separate provenance. Keep attribution and local assets together, without runtime icon requests or an entire-library dependency. Unknown-but-retained keys keep their existing fallback and persistence behavior.

The numbers type has 44 choices, including the official circled 0–9 series, new plain/square 0–9, and the previous numbering/sequence symbols. Separate sections provide all A–Z/a–z, uppercase/lowercase Roman 1–12, and 22 common/financial Chinese numeral glyphs. Search supports actual characters, Arabic values and meaningful Chinese/English names. Generic numbering-mode pictograms do not replace individually selectable characters.

Typographic outlines come from the pinned Noto Sans CJK SC Medium 2.004 source recorded in identityTypography.LICENSE.md. The reproducible generator checks its source hash, fits paths isotropically in the 24px viewBox and preserves a shared Latin baseline/x-height. Use precomposed Roman glyphs and vector Chinese shapes; no installed Windows font redistribution, runtime font dependency or text fallback. Keep their sourceKind separate from Fluent attribution; intentional semantic lookalikes such as Latin I and Roman I are allowed only in this character family. New saved keys use lowercase semantic prefixes to distinguish cases. Ship the complete copyright/OFL notice as public/LICENSE-NotoSansCJK.txt and verify Vite/desktop web output retains it.

The icon scroll viewport extends 16px into the existing right popup padding, with matching internal clearance preserving grid content width. This moves the shared overlay scrollbar's entire interaction rail beyond icon cells, retaining the normal 11-column density. Keep the global 5px thumb, 5px container-edge gap and 10px invisible rail/controller unchanged. Verify rightmost-cell hit tests and thumb dragging at normal/narrow/enlarged sizes, including inner-versus-outer scroll ownership, with no horizontal overflow or second scrollbar implementation.

Responsive fixtures must distinguish icon content width from the padded scroll-owner width. When constraining the owner directly, add its computed inline padding to the intended grid content width; retain the expected column and navigation assertions rather than shrinking their target geometry accidentally.

All icons and search results display labeled type sections in category order, with stable catalog order inside each section. Each icon belongs to exactly one type. Omit empty sections; a category filter shows only its matching section. Use compact, noninteractive headings and no additional card boundaries. All sections share one bounded icon scroll area, one radio group and one roving Tab entry. Keep an unknown current key available without assigning it to an unrelated type. ArrowUp/Down navigate adjacent rendered rows across headings and partial rows using viewport coordinates and nearest horizontal position; ArrowLeft/Right and Home/End follow visible grouped DOM order. Do not use flat catalog indices or section-relative offsets for grouped navigation.

Row equivalence must tolerate the existing small button hover translation and fractional zoom rounding while keeping adjacent rows distinct. Exact top-coordinate equality or a strict one-pixel threshold can split a hovered/clicked item into a false extra row. Use the cells' vertical geometry to identify a row, and cover mouse click followed by ArrowUp/Down in addition to keyboard-only navigation. Keep the shared hover treatment unchanged.

Selection and arrow navigation update local preview immediately. Apply submits one complete pair once; a synchronous pending guard prevents duplicate requests. Cancel, Escape and permitted outside dismissal discard this editor session. Reset remains inside the editor and changes the draft to `library/#ffffff`; it does not save independently. Save errors remain visible with the chosen draft and allow retry; close only after successful application. Browser network failures such as `TypeError: Failed to fetch` receive concise Chinese connection/retry text; preserve specific Core business reasons and useful unknown diagnostics. Freeze submission, draft editing and dismissal while committing.

Anchor the popup to its triggering control using the shared registered overlay stack and viewport collision handling. Keep its Library switcher or creation parent family alive; Settings uses the same registered popup. There is no centered appearance dialog or full-screen scrim. Opening/closing must preserve the parent state, and creation Apply updates its form preview without sending a metadata PATCH. Keep one keyboard owner and a stable focus-return target. Pending dismissal must be prevented before registry removal, including window blur and parent-family dismissal; guarding only the onDismiss callback is insufficient.

Offer a searchable, semantically varied icon collection, preset swatches, custom color selection and editable HEX input. Invalid custom input must be visible and prevent applying an unintended prior color. Local edits remain drafts until Apply; reset remains inside this popup. Preserve existing saved values, including colors outside the offered palette and unknown-but-retained glyph keys. Selecting only a glyph must not replace an existing custom color; selecting only a color must not silently normalize an untouched glyph key. Reopening starts from the latest authoritative pair (or the current creation draft), never a stale previous editor session.

After persistence, the active Library's titlebar and list must visibly change in the saving window and any already-open secondary window. Editing an inactive Library must not paint its appearance onto the active Library. Registry invalidation must not publish context phases or reset the media grid, selection or scroll. Responses crossing a Library switch, editor close/reopen, or unmount must not overwrite newer state.

## 4. Validation / error matrix

| Scenario | Required outcome |
| --- | --- |
| Dev HTTP renderer with a distinct Core origin | Real browser preflight permits PATCH; authenticated revision-bound save succeeds |
| Packaged renderer | Record actual renderer/Core origins and verify the distributed UI/binary, independently of Dev |
| Rapid glyph/color selection | Preview follows the latest draft; no metadata write before Apply |
| Apply twice / pending edits | One request; controls cannot mutate the submitted pair |
| Cancel / Escape / reopen | No write; reopen uses current saved values |
| Failed apply | Error and draft retained; retry succeeds without hidden dismissal |
| Settings save while main is open | Main titlebar/list update through registry invalidation |
| Inactive Library edit / switch during save | Active appearance remains authoritative; no stale repaint or media teardown |
| Creation editor Apply / Cancel | Parent draft changes only on Apply; no metadata request and no parent form loss |

## 5. Research and design decision

[React Aria's ColorSwatchPicker](https://react-aria.adobe.com/ColorSwatchPicker) demonstrates controlled color values, grid presentation and selected/disabled states. [Radix's Radio Group guidance](https://www.radix-ui.com/primitives/docs/components/radio-group) documents mutually exclusive choices, one group entry point and arrow-key navigation. [Notion's icon guidance](https://www.notion.com/en-gb/help/guides/page-icons-and-covers) illustrates a direct appearance-selection surface.

Mangoo adopts these selection patterns within its existing components. Separating local preview from an explicit Apply is a project design choice for editing the two related fields together; those references do not prescribe this commit policy. Keep compatible persisted glyph/color values while redesigning artwork, grouping, target geometry, feedback and save lifecycle. The identity artwork uses the official MIT-licensed [Fluent System Icons](https://github.com/microsoft/fluentui-system-icons), bundled locally; Microsoft product launch and trademark assets are excluded. This icon choice does not replace Mangoo's acrylic material or introduce a separate UI component framework.

## 6. Tests required

Add an actual router OPTIONS/PATCH preflight regression using the real required headers and current origin policy. Preserve the failing Dev network evidence and repeat with the corrected Core. Browser failures must be checked through network errors/status and visible outcome, not merely the absence of a thrown harness exception.

Mounted tests cover both independent radio groups, all choices, rapid drafts, one paired Apply, Cancel/Escape/reset, pending guards, failed retry, custom values, stale sessions, creation draft isolation, and the real global shortcut hook. Metadata tests verify registry publication only after success and consumer generation/unmount/switch fences.

Catalog/density/grouping changes additionally check unique keys, complete artwork pairs, Chinese/English search and category coverage, visible headings and membership without duplication, computed cell/glyph/gap geometry, responsive arrow movement across group boundaries and partial rows, and last-row reachability. Filtering and empty results must preserve valid keyboard entry and saved/unknown drafts. Actual desktop checks select a newly added identity and verify it after Apply/reopen while confirming unchanged consumer icon sizes.

Scrollbar clearance also requires actual hit-tested thumb and full-rail dragging inside the open appearance popup. Body-mounted global scrollbar elements must resolve to their live scroll owner through the shared overlay ownership bridge; the editor must remain open while its content scrolls. At enlarged sizes verify inner icon scrolling separately from outer body scrolling and HEX reachability. A standalone grid geometry fixture does not cover popup outside-click behavior.

Native isolated runs cover cross-origin Dev and packaged operation, visible titlebar/list/Settings synchronization, same-window and cross-window saves, normal pointer paths and keyboard navigation, and current OS-composited appearance/creation states. Record actual origins, loaded code and Core binary identity. Protect real user Libraries/processes and redact session credentials at collection time. Run final Web/Desktop/UI checks and focused Core regression/build checks; report any unavailable or untested path explicitly.

## 7. Wrong vs correct

Wrong: assert icon editing works because a direct API test changed metadata, while the real HTTP renderer's preflight is rejected; or close a menu after a save without updating the titlebar.

Correct: verify real renderer transport, one confirmed pair save, authoritative visible consumers and failure recovery. Local preview and saved state are distinct, and a failed request never becomes a success notification or an invisible discarded draft.
