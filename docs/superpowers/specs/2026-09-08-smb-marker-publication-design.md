# SMB-compatible synchronized-root marker publication

## Problem and boundary

The current two-folder OneDrive product verification has 48 source media files. The local root is enrolled, while the SMB root remains at `capturing-seed` with a durable operation-specific temporary marker. Ordinary same-directory rename succeeds on that share, but the current marker publication path fails. This design changes marker publication in `library_path.rs` and `sync_folders.rs`; enrollment ownership, UUIDs, Host/Catalog recovery and existing media remain authoritative.

The portable marker format and public API stay unchanged. Generic directory relocation and marker replacement are separate operations and must not inherit weaker semantics from this change. Normal product synchronization is the only authorized source of remote writes. No database edits, remote cleanup, source-media mutation or second-device restore are part of this verification.

## Publication protocol

1. Resolve the absolute local or UNC path into its volume/share anchor and ordinary child components. Open each component without following reparse points, relative to the previously opened component. Retain all component handles until publication and validation finish. Reject unsupported prefixes, traversal, alternate streams and reparse components.
2. Directory handles request only the rights needed for traversal, identity and marker work, without `DELETE` access. They permit read/write sharing but deny delete sharing. Include directory-list/read access so the guard is not an attribute-only open. The marker root may request write access for flush; generic `SafeDirectory` handles that request `DELETE` must not be retained against the same guarded root.
3. Verify the root handle identity against the enrollment's expected filesystem identity. Recovery never recreates a missing enrolled directory. A matching final marker is an idempotent success; a foreign marker is a conflict.
4. Create or reopen only `.mangoo-sync-root.<operationUuid>.tmp` belonging to the durable operation, with no-follow semantics. A Windows marker handle permits read sharing but denies write and delete sharing. Keep that same handle through exact-content validation, writing, `sync_all`, rename and final verification. A conflicting already-open writer/deleter makes acquisition fail before publication.
5. Publish using native `FileRenameInformation` with `RootDirectory = NULL`, a single target leaf name, and `ReplaceIfExists = FALSE`. This is the documented same-directory rename of the open source file. Do not use a full-path move, replacement flag, delete-and-recreate, or the former absolute-path fallback in this marker path.
6. Check final child identity and marker content using access compatible with the retained source handle, and revalidate the guarded root/path identity. Only then may enrollment advance to Host binding and Catalog projection. Windows remote directory flush may report the already-defined unsupported errors after file flush; unexpected failures remain failures.

## Why both guards are needed

The open source handle binds the rename to the verified file; denying write/delete sharing closes the validation-to-publication replacement window. Retained directory component guards prevent another compliant filesystem client from moving the root or one of its ancestors while a network redirector constructs the rename request. Post-publication identity checks alone cannot prevent a write into a replaced directory.

Windows same-directory native rename avoids an unnecessary target-directory handle/name mode. Directory guards deliberately omit `DELETE` access to avoid conflicting with one another. Compatibility must be demonstrated through the production helper on local NTFS, Windows UNC and the user's SMB share. An arbitrary server that does not enforce access/share rules is outside the filesystem trust model; failure to acquire or enforce required guards must never trigger an unguarded retry.

## Recovery and failure behavior

- Flush-before-rename preserves complete-or-absent publication under supported filesystem semantics.
- A failed rename retains the exact operation temporary marker. Recovery does not scan or adopt unrelated temporary files.
- Existing or racing target files are never overwritten, including a matching-content foreign file. A later whole-operation replay can recognize an already published matching final marker.
- Path, identity, reparse, sharing or content mismatches stop publication and do not create a binding or report completion.
- Diagnostics identify the failed stage and numeric OS error without source paths, marker contents, authentication material or remote URLs.
- Error 87 is not a signal to fall back to another rename API. The marker path does not need an unsupported-error fallback.

## Non-Windows compatibility boundary

Linux/Android publish with `renameat2(parent_fd, source_leaf, parent_fd, target_leaf, RENAME_NOREPLACE)`; Apple uses `renameatx_np(..., RENAME_EXCL)`. Other platforms return `Unsupported`. Do not emulate no-replace with hard-link/unlink: it adds a filesystem capability requirement unrelated to rename and breaks otherwise compatible volumes. An unsupported native no-replace operation remains a failure; never fall back to replacing rename.

The Linux publication syscall is relative to the captured directory, but Unix does not implement Windows deny-write/delete sharing. This is not a claim of identical cross-platform path-chain/source-write protection. WSL tests exercise the current `library_path.rs`, including target collision, directory-path replacement around the fd-relative syscall, and successful production-helper publication with `link/linkat` made unavailable. Full Core Linux builds and real FAT/exFAT, Apple, Android and Linux SMB mounts require separate evidence.

## Verification

Tests must use the same guarded helper as production, not just call a low-level API with a different handle configuration:

- fresh and exact-operation resumed marker publication on NTFS and local UNC;
- root and ancestor second `DELETE` opens/rename attempts rejected while guards live;
- source write, truncate, delete and move attempts rejected while pinned;
- a preexisting conflicting source/parent handle prevents publication;
- existing target, target race, wrong temporary identity/content and unrelated temporary markers remain untouched;
- paths replaced before guard acquisition, reparse components and invalid child names fail before publication;
- injected publication failure retains the persisted temporary marker and replay completes once;
- enrollment integration preserves root/sentinel identity and does not commit a binding after marker failure;
- ordinary relocation and non-marker filesystem tests retain their behavior.

After independent review and the full repository gate, rebuild the packaged product, replay the existing SMB enrollment through its normal API, then use the normal retry/sync action. Record both root inventories, publication queues, receipts, checkpoint/frontier and remote summary. Compare the 48 source files with the read-only baseline. A successful account poll or zero public queue alone is insufficient; raw pending-hash work and the remote root inventory must also converge. Second-device restore remains unverified.

## References

- [Microsoft WDK FILE_RENAME_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information): same-directory simple-name form and target-directory access semantics.
- [Microsoft CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew): desired access and sharing checks.
- Task research: `.trellis/tasks/09-07-verify-onedrive-two-folder-sync/research/marker-publish-design.md` and `live-baseline.md`.
