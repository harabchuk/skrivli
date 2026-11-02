@echo off
setlocal EnableExtensions

REM Format first-party C++ in place with clang-format (-i).
REM Uses the repo-root .clang-format (FormatStyle: file).
REM Usage: scripts\format.bat [file ...]
REM With no files, formats every C/C++ source under src\.

set "ROOT=%~dp0.."
set "FILE_LIST=%TEMP%\meetingbuddy_format_files.txt"
if exist "%FILE_LIST%" del /q "%FILE_LIST%"
set "HAS_FILES="

:parse_args
if "%~1"=="" goto :args_done
>>"%FILE_LIST%" echo %~f1
set "HAS_FILES=1"
shift
goto :parse_args

:args_done
cd /d "%ROOT%"

set "ARCH_DIR=x64"
if /I "%PROCESSOR_ARCHITECTURE%"=="ARM64" set "ARCH_DIR=ARM64"

call :find_llvm
if not defined CLANG_FORMAT (
    echo clang-format was not found. Install LLVM, or the Visual Studio component "C++ Clang tools for Windows".
    exit /b 1
)

echo Using clang-format: %CLANG_FORMAT%
echo Formatting sources...

set "FORMAT_FAIL=0"
if defined HAS_FILES (
    for /f "usebackq delims=" %%F in ("%FILE_LIST%") do call :format_listed "%%F"
) else (
    for /r "%ROOT%\src" %%F in (*.c *.cc *.cpp *.cxx *.h *.hh *.hpp *.hxx) do call :format_one "%%F"
)

if exist "%FILE_LIST%" del /q "%FILE_LIST%"
if "%FORMAT_FAIL%"=="1" exit /b 1

echo Format finished.
exit /b 0

:format_listed
if not exist "%~1" (
    echo File not found: %~1
    set "FORMAT_FAIL=1"
    exit /b 0
)
call :is_format_ext "%~x1"
if errorlevel 1 (
    echo Not a C or C++ source file: %~1
    set "FORMAT_FAIL=1"
    exit /b 0
)
call :format_one "%~1"
exit /b 0

:format_one
if not exist "%~1" exit /b 0
echo -- %~1
"%CLANG_FORMAT%" -i --style=file --fallback-style=none "%~1"
if errorlevel 1 (
    set "FORMAT_FAIL=1"
    echo clang-format failed: %~1
)
exit /b 0

:is_format_ext
if /I "%~1"==".c" exit /b 0
if /I "%~1"==".cc" exit /b 0
if /I "%~1"==".cpp" exit /b 0
if /I "%~1"==".cxx" exit /b 0
if /I "%~1"==".h" exit /b 0
if /I "%~1"==".hh" exit /b 0
if /I "%~1"==".hpp" exit /b 0
if /I "%~1"==".hxx" exit /b 0
exit /b 1

:find_llvm
set "CLANG_FORMAT="
where clang-format >nul 2>&1
if errorlevel 1 goto :find_llvm_vs
for /f "delims=" %%I in ('where clang-format') do (
    if not defined CLANG_FORMAT set "CLANG_FORMAT=%%I"
)
exit /b 0

:find_llvm_vs
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" exit /b 0
call :find_llvm_arch %ARCH_DIR%
if defined CLANG_FORMAT exit /b 0
if /I not "%ARCH_DIR%"=="x64" (
    call :find_llvm_arch x64
    if defined CLANG_FORMAT exit /b 0
)
if /I not "%ARCH_DIR%"=="ARM64" (
    call :find_llvm_arch ARM64
)
exit /b 0

:find_llvm_arch
set "LLVM_LIST=%TEMP%\meetingbuddy_llvm_%~1.txt"
"%VSWHERE%" -all -prerelease -products * -sort -find "VC\Tools\Llvm\%~1\bin\clang-format.exe" >"%LLVM_LIST%" 2>nul
if not exist "%LLVM_LIST%" exit /b 0
for /f "usebackq delims=" %%I in ("%LLVM_LIST%") do (
    if not defined CLANG_FORMAT set "CLANG_FORMAT=%%I"
)
exit /b 0
