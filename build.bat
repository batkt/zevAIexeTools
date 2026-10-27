@echo off
setlocal
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build_release.ps1" %*
if %errorlevel% neq 0 (
    echo.
    echo Build failed with error code %errorlevel%.
    pause
    exit /b %errorlevel%
)
pause
