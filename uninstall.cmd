@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\uninstall.ps1" %*
set "siphon_exit=%errorlevel%"
if "%~1"=="" pause
exit /b %siphon_exit%
