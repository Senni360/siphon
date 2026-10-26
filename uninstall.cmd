@echo off
setlocal
set "siphon_shell=powershell.exe"
where pwsh.exe >nul 2>nul
if not errorlevel 1 set "siphon_shell=pwsh.exe"
"%siphon_shell%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\uninstall.ps1" %*
set "siphon_exit=%errorlevel%"
if "%~1"=="" pause
exit /b %siphon_exit%
