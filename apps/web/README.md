# Mangoo Web UI

React UI shared by Electron desktop and future Web/Docker.

Responsibilities:

- real-folder tree presentation
- media grid and segmented/windowed browsing
- preview and keyboard interaction
- metadata, tag, task, and plugin views

The UI talks to Core through `contracts/core-api/openapi.yaml`. It must not directly access the filesystem.
