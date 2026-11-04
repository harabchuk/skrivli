@echo off
setlocal EnableExtensions

set "ROOT=%~dp0.."
set "BUILD_DIR=%ROOT%\build"
set "CMAKE="
set "CMAKE_LIST=%TEMP%\skrivli_cmake.txt"
set "CTEST="

cd /d "%ROOT%"

where cmake >nul 2>&1
if not errorlevel 1 (
    for /f "delims=" %%I in ('where cmake') do (
        set "CMAKE=%%I"
        goto :cmake_found
    )
)

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
    "%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.CMake.Project -find "**\cmake.exe" >"%CMAKE_LIST%" 2>nul
    if exist "%CMAKE_LIST%" (
        for /f "usebackq delims=" %%I in ("%CMAKE_LIST%") do (
            set "CMAKE=%%I"
            goto :cmake_found
        )
    )
)

if exist "%ProgramFiles%\CMake\bin\cmake.exe" set "CMAKE=%ProgramFiles%\CMake\bin\cmake.exe" & goto :cmake_found

:cmake_found
if not defined CMAKE (
    echo cmake was not found.
    exit /b 1
)

for %%I in ("%CMAKE%") do set "CTEST=%%~dpIctest.exe"
if not exist "%CTEST%" set "CTEST=ctest"

if not exist "%BUILD_DIR%\CMakeCache.txt" (
    echo Configuring CMake...
    "%CMAKE%" -S . -B "%BUILD_DIR%" -A x64
    if errorlevel 1 exit /b 1
)

echo Building tests...
"%CMAKE%" --build "%BUILD_DIR%" --config Release --target SkrivliTests
if errorlevel 1 exit /b 1

echo Running ctest...
"%CTEST%" --test-dir "%BUILD_DIR%" -C Release --output-on-failure
exit /b %ERRORLEVEL%
