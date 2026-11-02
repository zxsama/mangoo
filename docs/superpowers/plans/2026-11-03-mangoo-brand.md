# Mangoo identity and application icon

The application, public packages, Core transport, local storage markers, plugin identity and provider-owned remote namespace use Mangoo consistently. This is an intentional identity break, with no old-name aliases, fallback reads or data migration. Existing user Libraries, profiles, credentials and remote objects are left untouched.

The two maintained projects become `D:\Mangoo` and `D:\Mangoo_OneDriveSync`. The main repository uses `https://github.com/zxsama/mangoo`. Source and maintained documentation names change together. Git history and preserved raw diagnostic snapshots retain their original bytes; name normalization in older documentation does not constitute fresh validation of the renamed application.

The editable icon is `apps/web/public/favicon.svg`: a golden mango silhouette, green leaf and dark M. It has no surrounding plate, thin outline or small text. The same vector supplies the browser favicon and generated Windows PNG/ICO assets at 16, 24, 32, 48, 64, 128 and 256 pixels. Generic interface icons and user-selected Library/folder icons remain independent of the application mark.

Implementation order: update contracts and both source trees; rebuild and pack the SDK; rebuild the independent plugin; regenerate artifact hashes and Core trust through the normal tooling; rebuild Core, Web and Desktop; verify clean isolated runtime behavior and icons; remove disposable stale outputs; move the project directories; verify from their new paths; review, commit and synchronize the main repository.

Validation includes both projects' normal checks, Windows packaging, branded startup and plugin transport. Real account operations, legacy data migration and unsupported platform release claims are outside this change.
