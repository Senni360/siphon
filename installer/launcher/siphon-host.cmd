@echo off
setlocal DisableDelayedExpansion
chcp 65001 >nul
cd /d "__HOST_DIR__"
if errorlevel 1 exit /b 1
set "PATH=__EXTRA_PATH__;%PATH%"
"__PYTHON__" -u -m siphon_host
exit /b %errorlevel%
