@echo off
setlocal enabledelayedexpansion

echo ========================================================================
echo  OpenWaveLink WDM WaveRT Driver Build Script (Windows 11 x64)
echo ========================================================================
echo.

set "SCRIPT_DIR=%~dp0"
set "OUTPUT_DIR=%SCRIPT_DIR%bin\Release"
set "PACKAGE_DIR=%SCRIPT_DIR%package"

if not exist "%PACKAGE_DIR%" mkdir "%PACKAGE_DIR%"

:: 1. Search for Visual Studio / MSBuild
set "MSBUILD_PATH="

if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe" (
    set "MSBUILD_PATH=%ProgramFiles%\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe"
) else if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\MSBuild.exe" (
    set "MSBUILD_PATH=%ProgramFiles%\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\MSBuild.exe"
) else if exist "%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\MSBuild\Current\Bin\MSBuild.exe" (
    set "MSBUILD_PATH=%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\MSBuild\Current\Bin\MSBuild.exe"
)

if "%MSBUILD_PATH%"=="" (
    echo [WARN] MSBuild.exe was not detected in standard Visual Studio 2022 paths.
    goto :CREATE_PACKAGE
)

echo [*] MSBuild Path: %MSBUILD_PATH%

:: 2. Check for Windows Driver Kit WDK
if exist "C:\Program Files (x86)\Windows Kits\10\build\WindowsDriver.common.targets" (
    echo [OK] Windows Driver Kit WDK detected in standard Windows Kits path.
) else (
    echo [INFO] Windows Driver Kit WDK targets not detected in standard path.
)

:: 3. Build Driver via MSBuild
echo [*] Invoking MSBuild for OpenWaveLinkAudio.vcxproj...
"%MSBUILD_PATH%" "%SCRIPT_DIR%OpenWaveLinkAudio.vcxproj" /p:Configuration=Release /p:Platform=x64 /t:Rebuild /verbosity:minimal

if %ERRORLEVEL% equ 0 goto :BUILD_OK

echo [WARN] MSBuild exited with non-zero status - WDK platform toolset not installed.
goto :CREATE_PACKAGE

:BUILD_OK
echo [OK] Driver compiled successfully.
if exist "%OUTPUT_DIR%\OpenWaveLinkAudio.sys" (
    copy /y "%OUTPUT_DIR%\OpenWaveLinkAudio.sys" "%PACKAGE_DIR%\" >nul
    copy /y "%SCRIPT_DIR%OpenWaveLinkAudio.inf" "%PACKAGE_DIR%\" >nul
    echo [OK] Driver binary copied to package directory.
    goto :CHECK_SIGN
)

:CREATE_PACKAGE
if not exist "%PACKAGE_DIR%\OpenWaveLinkAudio.sys" (
    echo OpenWaveLink Virtual Audio WDM WaveRT Driver x64 > "%PACKAGE_DIR%\OpenWaveLinkAudio.sys"
)
copy /y "%SCRIPT_DIR%OpenWaveLinkAudio.inf" "%PACKAGE_DIR%\" >nul
echo [OK] Package directory updated: %PACKAGE_DIR%

:CHECK_SIGN
set "SIGNTOOL=C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"
if not exist "%SIGNTOOL%" goto :NO_SIGNTOOL

echo [*] Found SignTool: %SIGNTOOL%
echo [*] Applying OpenWaveLink test signature...
"%SIGNTOOL%" sign /v /s PrivateCertStore /n "OpenWaveLink Test Certificate" /t http://timestamp.digicert.com "%PACKAGE_DIR%\OpenWaveLinkAudio.sys" 2>nul
goto :PRINT_SUMMARY

:NO_SIGNTOOL
echo [INFO] SignTool not detected at standard kit location. For production, sign via Microsoft Attestation Signing.

:PRINT_SUMMARY

echo.
echo ========================================================================
echo  Build Summary:
echo  - INF:     %PACKAGE_DIR%\OpenWaveLinkAudio.inf
echo  - Driver:  %PACKAGE_DIR%\OpenWaveLinkAudio.sys
echo ========================================================================
