@echo off
setlocal
cd /d "%~dp0"

call tools\dev\ensure-desktop-deps.cmd
if errorlevel 1 exit /b %errorlevel%

call npm.cmd run dev
exit /b %errorlevel%
