@echo off
setlocal
cd /d "%~dp0"

node tools/dev/run-web.mjs
