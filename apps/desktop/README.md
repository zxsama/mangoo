# Mangoo Desktop

Electron shell for Windows desktop.

Responsibilities:

- create and manage native windows
- start/stop the Core service
- pass a local session token to the renderer
- expose native dialogs through controlled IPC
- keep desktop-only behavior out of the Web UI

Do not put media indexing, thumbnail generation, database access, or file operations here.
