@echo off
setlocal
cd /d "%~dp0"

REM One-click production launcher for the Megle desktop app.
REM   - No source changes since the last package -> opens Megle.exe immediately.
REM   - Source changed (or outputs missing) -> builds release artifacts and packages Megle.exe.
REM   - The build shell exits after launch; the packaged app owns Core and Provider processes.
REM Pass a folder to auto-add on first launch:  start-desktop-prod.bat "D:\Photos"

if not "%~1"=="" set "MEGLE_AUTO_ADD_ROOT=%~1"

node tools\dev\run-prod.mjs
exit /b %errorlevel%
