@echo off
setlocal EnableExtensions EnableDelayedExpansion

set "ROOT=%~dp0.."
set "BUILD_DIR=%ROOT%\build"
set "DIST_DIR=%ROOT%\dist"
set "CONFIG=Release"
set "EXE=%BUILD_DIR%\%CONFIG%\Skrivli.exe"
set "README=%ROOT%\README.md"
set "VERSION=1.0.0"
set "STAGE=%DIST_DIR%\Skrivli-%VERSION%"
set "ZIP=%DIST_DIR%\Skrivli-%VERSION%.zip"
set "CMAKE="
set "CMAKE_LIST=%TEMP%\skrivli_cmake.txt"

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

echo Stopping Skrivli if it is running...
taskkill /IM Skrivli.exe /F >nul 2>&1
if errorlevel 1 (
    echo Skrivli was not running.
) else (
    echo Skrivli stopped.
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

echo Building Skrivli (%CONFIG%)...
"%CMAKE%" --build "%BUILD_DIR%" --config %CONFIG% --target Skrivli
if errorlevel 1 (
    echo Build failed.
    exit /b 1
)

if not exist "%EXE%" (
    echo Executable not found: %EXE%
    exit /b 1
)

if not exist "%README%" (
    echo README.md not found: %README%
    exit /b 1
)

echo Staging portable package...
if exist "%STAGE%" rmdir /s /q "%STAGE%"
if exist "%ZIP%" del /f /q "%ZIP%"
mkdir "%DIST_DIR%" >nul 2>&1
mkdir "%STAGE%"
if errorlevel 1 (
    echo Failed to create staging directory.
    exit /b 1
)

copy /y "%EXE%" "%STAGE%\Skrivli.exe" >nul
if errorlevel 1 (
    echo Failed to copy Skrivli.exe
    exit /b 1
)
copy /y "%README%" "%STAGE%\README.md" >nul
if errorlevel 1 (
    echo Failed to copy README.md
    exit /b 1
)

REM Copy non-system DLLs that sit next to the exe (none expected with the static whisper build).
for %%F in ("%BUILD_DIR%\%CONFIG%\*.dll") do (
    set "NAME=%%~nxF"
    set "SKIP=0"
    if /I "!NAME!"=="SkrivliTests.exe" set "SKIP=1"
    echo !NAME! | findstr /I /R "^api-ms- ^ucrtbase ^vcruntime ^msvcp ^concrt ^vccorlib" >nul
    if not errorlevel 1 set "SKIP=1"
    if "!SKIP!"=="0" (
        echo Including %%~nxF
        copy /y "%%F" "%STAGE%\%%~nxF" >nul
    )
)

echo Creating %ZIP%...
powershell -NoProfile -Command "Compress-Archive -Path '%STAGE%\*' -DestinationPath '%ZIP%' -Force"
if errorlevel 1 (
    echo Zip failed.
    exit /b 1
)

echo Packed: %ZIP%
dir "%ZIP%"
endlocal
exit /b 0
