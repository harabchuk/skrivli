@echo off
setlocal

rem Run from the repository root regardless of where the script is launched.
pushd "%~dp0.."

taskkill /F /IM skrivli.exe >nul 2>&1
if %errorlevel%==0 (echo Stopped running skrivli.exe) else (echo skrivli.exe was not running)

cargo build
if errorlevel 1 (
    echo Build failed.
    popd
    exit /b 1
)

rem Launch detached so the script exits right after starting the app.
start "" target\debug\skrivli.exe

popd
exit /b 0
