@echo off
setlocal
cd /d "%~dp0..\.."

if exist "node_modules\.package-lock.json" if exist "node_modules\electron-builder\package.json" exit /b 0

echo [Mangoo] Installing desktop dependencies from package-lock.json...
call npm.cmd ci --include=dev
exit /b %errorlevel%
