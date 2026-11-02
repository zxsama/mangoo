# Library creation layout refinement

Completed 2026-11-03. The user chose direct editing without creating a Trellis task.

## Final behavior

- A unified icon/name bar replaces the separately aligned icon, label and input. It owns one boundary and pointer highlight; the input retains its accessible name and field-level errors below the bar.
- Material folders are the primary form section. Each row shows its folder name above the complete editable path, with compact selection/removal actions inside one boundary.
- Database is a quiet full-width disclosure below the folder list. Desktop starts collapsed: `数据库` at the left, a muted current-path summary in the middle and the chevron at the right. Long paths ellipsize while the complete tooltip/input value remains available. Web starts expanded for required Core-host input. Database/resolver failures reveal the editor before focus and scrolling recover to it. Visible section/action copy is `数据库` / `文件夹` / `添加`.
- Name/path drafts, native selection, multiple roots, minimum-one validation, immutable preflight/create, busy locks and async-generation fences remain intact. No Core/API/native protocol or icon-library change.
- Existing glass, 20px/28px title and 14px/20px body typography, fixed actions, appearance popup and single-boundary rules are preserved. Routine hints and repeated change buttons were removed.

The unified name bar supersedes the intermediate separate-label alignment. A subsequent explicit screenshot request restored the collapsed Database path summary while retaining its low visual priority, and shortened the three labels. Current contracts are in `docs/superpowers/specs/2026-10-31-onboarding-and-icons.md` and `2026-11-02-library-storage-separation.md`.

## Verification

Final Web typecheck, UI-design guard and complete mounted creation workflow passed. The mounted test retains all prior payload/lifecycle/async checks and covers unified boundaries, name-error accessibility, Desktop collapse, Web default expansion, Database/resolver error expansion and focus, keyboard/Escape, appearance, root editing and constrained geometry. Final runtime: 26.2 seconds.

Earlier in this same follow-up, full Web 528/528 and Desktop 237/237 passed. They were not repeated after the final local identity/disclosure refinements; the affected mounted workflow, typecheck and UI guard were rerun. Core was unchanged, so no Rust/full-project gate was repeated.

Actual isolated Electron verification confirmed 16 outcomes, including creation/activation while Database remained collapsed, authoritative path/name errors, picker cancellation and busy protection, appearance/Escape, and normal/narrow/125/150 layout. It used private `.tmp` Library/Host/profile data and the previously verified unchanged Core (`6e4703f1467f2c84580718c1b6bb48eadb64f83996ce45b0fddc0bd3817d0a64`). No user Documents Library was created or changed.

Final evidence is in `.tmp/creation-ui-layout-20261103/check-report.md` and its `native-run/` directory:

- `final-disclosure-desktop-os.png`: actual OS-composited owned window; main independently inspected it.
- `final-disclosure-normal-renderer.png`, `final-disclosure-narrow-renderer.png`, `final-disclosure-125-renderer.png`, `final-disclosure-150-renderer.png`.
- `final-disclosure-expanded-renderer.png`, name/Database error captures and appearance captures.

Native 125/150 captures use device-metrics emulation; the mounted workflow separately uses Electron zoom. Neither proves physical Windows DPI or display anti-aliasing. Earlier alignment/identity/compound/design images are intermediate, not final delivery evidence.

### Reference-summary and label follow-up

After the native run above, the user explicitly selected the summary-row version and the wording `数据库` / `文件夹` / `添加`. This local JSX/CSS update retained state, defaults, focus recovery and payloads. Fresh typecheck, UI-design guard and the entire mounted workflow passed (25.46 seconds), including new fixed-label/right-chevron/390px ellipsis checks, full tooltip equality and absence of duplicated expanded summary. Main inspected current normal and narrow captures.

Latest visual evidence: `.tmp/creation-ui-summary-20261103/captures/summary-normal-collapsed.png`, `summary-normal-expanded.png`, `summary-narrow-collapsed.png` and `summary-narrow-expanded.png`; scoped baseline and verification are in that directory's `result.md`. These renderer captures supersede the earlier native screenshots for the final summary/copy. The unchanged native material/lifecycle was not retested for this small follow-up; no fresh compositor or physical-AA claim is made for it.

## Scope and cleanup

Product/test edits are limited to `ManagedLibraryWizard.tsx`, its local `managed-library-*` CSS, the existing mounted runner and its fixture's controllable resolver failure. Two current design specifications were updated. A 403-file pre-existing dirty inventory and exact local file backups were preserved. Final audit found only these six pre-existing paths changed; staging stayed identical. The verification report is the additional new documentation file. No task, commit or broad cleanup was created.

Owned test processes exited; user main Electron/Core remained running. Private fixtures and screenshots remain under ignored `.tmp` as evidence. Full logs, process identities, source hashes and minor harness corrections are retained in the detailed check report.
