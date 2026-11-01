@echo off
setlocal EnableExtensions

set "ROOT=%~dp0.."
set "BUILD_DIR=%ROOT%\build"
set "EXE=%BUILD_DIR%\Release\MeetingBuddy.exe"
set "CONFIG=Release"
set "CMAKE="
set "CMAKE_LIST=%TEMP%\meetingbuddy_cmake.txt"

cd /d "%ROOT%"

REM Prefer cmake on PATH.
where cmake >nul 2>&1
if not errorlevel 1 (
    for /f "delims=" %%I in ('where cmake') do (
        set "CMAKE=%%I"
        goto :cmake_found
    )
)

REM Then Visual Studio / Build Tools CMake via vswhere.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
    "%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.CMake.Project -find "**\cmake.exe" >"%CMAKE_LIST%" 2>nul
    if exist "%CMAKE_LIST%" (
        for /f "usebackq delims=" %%I in ("%CMAKE_LIST%") do (
            set "CMAKE=%%I"
            goto :cmake_found
        )
    )
    "%VSWHERE%" -latest -products * -find "**\CMake\bin\cmake.exe" >"%CMAKE_LIST%" 2>nul
    if exist "%CMAKE_LIST%" (
        for /f "usebackq delims=" %%I in ("%CMAKE_LIST%") do (
            set "CMAKE=%%I"
            goto :cmake_found
        )
    )
)

if exist "%ProgramFiles%\CMake\bin\cmake.exe" set "CMAKE=%ProgramFiles%\CMake\bin\cmake.exe" & goto :cmake_found
if exist "%ProgramFiles(x86)%\CMake\bin\cmake.exe" set "CMAKE=%ProgramFiles(x86)%\CMake\bin\cmake.exe" & goto :cmake_found
if exist "%LOCALAPPDATA%\Programs\CMake\bin\cmake.exe" set "CMAKE=%LOCALAPPDATA%\Programs\CMake\bin\cmake.exe" & goto :cmake_found

:cmake_found
if not defined CMAKE (
    echo cmake was not found. Install CMake or Visual Studio Build Tools with the CMake component.
    exit /b 1
)

echo Using cmake: %CMAKE%

echo Stopping MeetingBuddy if it is running...
taskkill /IM MeetingBuddy.exe /F >nul 2>&1
if errorlevel 1 (
    echo MeetingBuddy was not running.
) else (
    echo MeetingBuddy stopped.
    timeout /t 1 /nobreak >nul
)

if not exist "%BUILD_DIR%\CMakeCache.txt" (
    echo Configuring CMake...
    "%CMAKE%" -S . -B "%BUILD_DIR%" -A x64
    if errorlevel 1 (
        echo CMake configure failed.
        exit /b 1
    )
)

echo Building MeetingBuddy (%CONFIG%)...
"%CMAKE%" --build "%BUILD_DIR%" --config %CONFIG%
if errorlevel 1 (
    echo Build failed.
    exit /b 1
)

if not exist "%EXE%" (
    echo Executable not found: %EXE%
    exit /b 1
)

echo Starting MeetingBuddy...
start "" "%EXE%"
endlocal
