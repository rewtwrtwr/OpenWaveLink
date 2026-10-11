@echo off
setlocal enabledelayedexpansion

echo ========================================================================
echo  OpenWaveLink Windows 11 Release Build Pipeline (One-Click Deploy)
echo  Components: React UI + Rust 2024 Core + WDM WaveRT Driver + Installer
echo ========================================================================
echo.

set "ROOT_DIR=%~dp0"
cd /d "%ROOT_DIR%"

:: 1. Build React Fluent UI (Vite + TypeScript)
echo [1/4] Building React Fluent UI frontend...
call pnpm --prefix crates\owl-app\ui run build
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Frontend build failed with code %ERRORLEVEL%!
    exit /b %ERRORLEVEL%
)
echo [OK] Frontend built successfully: crates\owl-app\ui\dist
echo.

:: 2. Build Rust 2024 Workspace Binaries (Release Mode)
echo [2/4] Compiling Rust 2024 Workspace in Release mode...
call cargo build --release --workspace
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Cargo release build failed with code %ERRORLEVEL%!
    exit /b %ERRORLEVEL%
)

if not exist "target\release\owl-daemon.exe" (
    echo [ERROR] owl-daemon.exe was not found in target\release!
    exit /b 1
)
if not exist "target\release\owl-app.exe" (
    echo [ERROR] owl-app.exe was not found in target\release!
    exit /b 1
)
echo [OK] Binaries generated: target\release\owl-daemon.exe ^& target\release\owl-app.exe
echo.

:: 3. Build & Package WDM WaveRT Kernel Audio Driver
echo [3/4] Packaging WDM WaveRT Virtual Audio Driver...
call driver\build_driver.bat
if not exist "driver\package\OpenWaveLinkAudio.inf" (
    echo [ERROR] Driver INF missing in driver\package!
    exit /b 1
)
if not exist "driver\package\OpenWaveLinkAudio.sys" (
    echo [ERROR] Driver SYS missing in driver\package!
    exit /b 1
)
echo [OK] Driver package ready: driver\package\
echo.

:: 4. Build Windows 11 Setup Installer (Inno Setup)
echo [4/4] Creating Windows 11 Distribution Installer...

set "ISCC="
if exist "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" (
    set "ISCC=C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
) else if exist "C:\Program Files\Inno Setup 6\ISCC.exe" (
    set "ISCC=C:\Program Files\Inno Setup 6\ISCC.exe"
) else if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" (
    set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
)

:: Prepare staged release payload directory
set "PAYLOAD_DIR=installer\release_payload"
if not exist "%PAYLOAD_DIR%" mkdir "%PAYLOAD_DIR%"
if not exist "%PAYLOAD_DIR%\driver" mkdir "%PAYLOAD_DIR%\driver"
if not exist "%PAYLOAD_DIR%\ui" mkdir "%PAYLOAD_DIR%\ui"

copy /y "target\release\owl-app.exe" "%PAYLOAD_DIR%\" >nul
copy /y "target\release\owl-daemon.exe" "%PAYLOAD_DIR%\" >nul
if exist "bin\virtualcables.exe" copy /y "bin\virtualcables.exe" "%PAYLOAD_DIR%\" >nul
if not exist "%PAYLOAD_DIR%\bin" mkdir "%PAYLOAD_DIR%\bin"
if exist "bin\virtualcables.exe" copy /y "bin\virtualcables.exe" "%PAYLOAD_DIR%\bin\" >nul
copy /y "driver\package\OpenWaveLinkAudio.inf" "%PAYLOAD_DIR%\driver\" >nul
copy /y "driver\package\OpenWaveLinkAudio.sys" "%PAYLOAD_DIR%\driver\" >nul
xcopy /s /e /y "crates\owl-app\ui\dist\*" "%PAYLOAD_DIR%\ui\" >nul

:: Create driver helper scripts in payload
echo @echo off > "%PAYLOAD_DIR%\install_driver.bat"
echo pnputil.exe /add-driver "%%~dp0driver\OpenWaveLinkAudio.inf" /install >> "%PAYLOAD_DIR%\install_driver.bat"
echo pause >> "%PAYLOAD_DIR%\install_driver.bat"

echo @echo off > "%PAYLOAD_DIR%\uninstall_driver.bat"
echo pnputil.exe /delete-driver OpenWaveLinkAudio.inf /uninstall /force >> "%PAYLOAD_DIR%\uninstall_driver.bat"
echo pause >> "%PAYLOAD_DIR%\uninstall_driver.bat"

if "%ISCC%"=="" goto :NO_ISCC

echo [*] Found Inno Setup Compiler: %ISCC%
"%ISCC%" "%ROOT_DIR%installer\OpenWaveLink_Setup.iss"
if %ERRORLEVEL% equ 0 (
    echo [OK] Installer successfully compiled: installer\dist\OpenWaveLink_v1.0.0_Setup.exe
    goto :RELEASE_COMPLETE
) else (
    echo [WARN] Inno Setup compilation exited with code %ERRORLEVEL%.
    goto :NO_ISCC
)

:NO_ISCC
echo [INFO] Inno Setup 6 compiler (ISCC.exe) not installed or detected in default paths.
echo        Staged standalone release package generated in: %PAYLOAD_DIR%
echo        To build the single .exe installer, install Inno Setup 6 (https://jrsoftware.org/isdl.php)
echo        and run: ISCC.exe installer\OpenWaveLink_Setup.iss

:RELEASE_COMPLETE
echo.
echo ========================================================================
echo  OpenWaveLink Build Complete!
echo  - Payload:   %PAYLOAD_DIR%\
echo  - Daemon:    %PAYLOAD_DIR%\owl-daemon.exe
echo  - Client:    %PAYLOAD_DIR%\owl-app.exe
echo  - Driver:    %PAYLOAD_DIR%\driver\OpenWaveLinkAudio.inf (.sys)
echo  - UI Assets: %PAYLOAD_DIR%\ui\
echo ========================================================================
