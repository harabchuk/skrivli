@echo off
setlocal EnableExtensions

REM Format first-party C++ with clang-format.
REM clang-tidy is commented out for now.
REM Usage: scripts\lint.bat

set "ROOT=%~dp0.."
cd /d "%ROOT%"

set "ARCH_DIR=x64"
if /I "%PROCESSOR_ARCHITECTURE%"=="ARM64" set "ARCH_DIR=ARM64"

call :find_llvm
if not defined CLANG_FORMAT (
    echo clang-format was not found. Install LLVM, or the Visual Studio component "C++ Clang tools for Windows".
    exit /b 1
)
REM if not defined CLANG_TIDY (
REM     echo clang-tidy was not found. Install LLVM, or the Visual Studio component "C++ Clang tools for Windows".
REM     exit /b 1
REM )

echo Using clang-format: %CLANG_FORMAT%
REM echo Using clang-tidy: %CLANG_TIDY%

echo Formatting sources...
set "FORMAT_FAIL=0"
for /r "%ROOT%\src" %%F in (*.c *.cc *.cpp *.cxx *.h *.hh *.hpp *.hxx) do call :format_one "%%F"
call :format_one "%ROOT%\resources\resource.h"
if "%FORMAT_FAIL%"=="1" exit /b 1

echo Lint finished.
exit /b 0

REM clang-tidy is disabled for now.
REM set "COMPILE_DB=build-ninja\compile_commands.json"
REM set "NEED_CONFIG=0"
REM if /I "%~1"=="reconfigure" set "NEED_CONFIG=1"
REM if not exist "%COMPILE_DB%" set "NEED_CONFIG=1"
REM if "%NEED_CONFIG%"=="1" goto :run_configure
REM
REM powershell -NoProfile -Command "exit [int]((Get-Item -LiteralPath 'CMakeLists.txt','CMakePresets.json' | Sort-Object LastWriteTime -Descending | Select-Object -First 1).LastWriteTime -gt (Get-Item -LiteralPath 'build-ninja\compile_commands.json').LastWriteTime)"
REM if errorlevel 1 set "NEED_CONFIG=1"
REM
REM :run_configure
REM if "%NEED_CONFIG%"=="0" goto :tidy
REM
REM echo Configuring the ninja preset to generate compile_commands.json...
REM call :configure_ninja
REM if errorlevel 1 exit /b 1
REM if not exist "%COMPILE_DB%" (
REM     echo compile_commands.json was not generated.
REM     exit /b 1
REM )
REM
REM :tidy
REM call :ensure_vcvars
REM if errorlevel 1 exit /b 1
REM
REM echo Running clang-tidy...
REM set "TIDY_FAIL=0"
REM for /r "%ROOT%\src" %%F in (*.cpp *.cxx *.cc) do call :tidy_one "%%F"
REM if "%TIDY_FAIL%"=="1" exit /b 1

:format_one
if not exist "%~1" exit /b 0
"%CLANG_FORMAT%" -i --style=file "%~1"
if errorlevel 1 goto :format_one_failed
exit /b 0
:format_one_failed
set "FORMAT_FAIL=1"
echo clang-format failed: %~1
exit /b 0

:tidy_one
echo -- %~1
"%CLANG_TIDY%" --config-file="%ROOT%\.clang-tidy" -p "%ROOT%\build-ninja" --extra-arg-before=--driver-mode=cl "%~1"
if errorlevel 1 goto :tidy_one_failed
exit /b 0
:tidy_one_failed
set "TIDY_FAIL=1"
echo clang-tidy failed: %~1
exit /b 0

:find_llvm
set "CLANG_FORMAT="
set "CLANG_TIDY="
where clang-format >nul 2>&1
if errorlevel 1 goto :find_llvm_vs
where clang-tidy >nul 2>&1
if errorlevel 1 goto :find_llvm_vs
for /f "delims=" %%I in ('where clang-format') do (
    if not defined CLANG_FORMAT set "CLANG_FORMAT=%%I"
)
for /f "delims=" %%I in ('where clang-tidy') do (
    if not defined CLANG_TIDY set "CLANG_TIDY=%%I"
)
exit /b 0

:find_llvm_vs
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" exit /b 0
call :find_llvm_arch %ARCH_DIR%
if defined CLANG_TIDY exit /b 0
if /I not "%ARCH_DIR%"=="x64" (
    call :find_llvm_arch x64
    if defined CLANG_TIDY exit /b 0
)
if /I not "%ARCH_DIR%"=="ARM64" (
    call :find_llvm_arch ARM64
)
exit /b 0

:find_llvm_arch
set "CLANG_FORMAT="
set "CLANG_TIDY="
set "LLVM_LIST=%TEMP%\meetingbuddy_llvm_%~1.txt"
"%VSWHERE%" -all -prerelease -products * -sort -find "VC\Tools\Llvm\%~1\bin\clang-format.exe" >"%LLVM_LIST%" 2>nul
if not exist "%LLVM_LIST%" exit /b 0
for /f "usebackq delims=" %%I in ("%LLVM_LIST%") do (
    if not defined CLANG_FORMAT set "CLANG_FORMAT=%%I"
)
if not defined CLANG_FORMAT exit /b 0
for %%I in ("%CLANG_FORMAT%") do set "CLANG_TIDY=%%~dpIclang-tidy.exe"
if not exist "%CLANG_TIDY%" (
    set "CLANG_FORMAT="
    set "CLANG_TIDY="
)
exit /b 0

:find_cmake
set "CMAKE="
set "CMAKE_LIST=%TEMP%\meetingbuddy_cmake.txt"
where cmake >nul 2>&1
if errorlevel 1 goto :find_cmake_vs
for /f "delims=" %%I in ('where cmake') do (
    if not defined CMAKE set "CMAKE=%%I"
)
exit /b 0

:find_cmake_vs
if not exist "%VSWHERE%" set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" goto :find_cmake_fallback
"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.CMake.Project -find "**\cmake.exe" >"%CMAKE_LIST%" 2>nul
if exist "%CMAKE_LIST%" (
    for /f "usebackq delims=" %%I in ("%CMAKE_LIST%") do (
        if not defined CMAKE set "CMAKE=%%I"
    )
)
if defined CMAKE exit /b 0
"%VSWHERE%" -latest -products * -find "**\CMake\bin\cmake.exe" >"%CMAKE_LIST%" 2>nul
if exist "%CMAKE_LIST%" (
    for /f "usebackq delims=" %%I in ("%CMAKE_LIST%") do (
        if not defined CMAKE set "CMAKE=%%I"
    )
)
if defined CMAKE exit /b 0

:find_cmake_fallback
if exist "%ProgramFiles%\CMake\bin\cmake.exe" set "CMAKE=%ProgramFiles%\CMake\bin\cmake.exe" & exit /b 0
if exist "%ProgramFiles(x86)%\CMake\bin\cmake.exe" set "CMAKE=%ProgramFiles(x86)%\CMake\bin\cmake.exe" & exit /b 0
if exist "%LOCALAPPDATA%\Programs\CMake\bin\cmake.exe" set "CMAKE=%LOCALAPPDATA%\Programs\CMake\bin\cmake.exe" & exit /b 0
exit /b 0

:ensure_vcvars
if defined MEETINGBUDDY_VCVARS exit /b 0
if not defined VSWHERE set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VSINSTALL="
set "VS_LIST=%TEMP%\meetingbuddy_vs.txt"
"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath >"%VS_LIST%" 2>nul
if exist "%VS_LIST%" (
    for /f "usebackq delims=" %%I in ("%VS_LIST%") do (
        if not defined VSINSTALL set "VSINSTALL=%%I"
    )
)
if not defined VSINSTALL (
    echo Visual Studio C++ tools were not found. clang-tidy needs the MSVC include environment.
    exit /b 1
)
call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 (
    echo vcvars64.bat failed.
    exit /b 1
)
set "MEETINGBUDDY_VCVARS=1"
exit /b 0

:configure_ninja
call :ensure_vcvars
if errorlevel 1 exit /b 1
call :find_cmake
if not defined CMAKE (
    echo cmake was not found. Install CMake or Visual Studio Build Tools with the CMake component.
    exit /b 1
)

set "NINJA=%VSINSTALL%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe"
if exist "%NINJA%" goto :ninja_found
echo ninja was not found under the Visual Studio install.
exit /b 1
:ninja_found

echo Using Visual Studio: %VSINSTALL%
echo Using cmake: %CMAKE%
echo Using ninja: %NINJA%
for %%I in ("%NINJA%") do set "PATH=%%~dpI;%PATH%"
"%CMAKE%" --preset ninja
if errorlevel 1 (
    echo CMake configure failed.
    exit /b 1
)
exit /b 0
