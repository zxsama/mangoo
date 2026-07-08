@echo off
setlocal
cd /d "%~dp0"

REM One-click production launcher for the Megle desktop app.
REM   - No source changes since the last build -> skips compilation, opens immediately.
REM   - Source changed (or build outputs missing) -> builds release Core + web + desktop, then opens.
REM Pass a folder to auto-add on first launch:  start-desktop-prod.bat "D:\Photos"

if not "%~1"=="" set "MEGLE_AUTO_ADD_ROOT=%~1"

node tools\dev\run-prod.mjs
