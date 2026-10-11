@echo off
setlocal enabledelayedexpansion

echo ========================================================================
echo  OpenWaveLink - Сборка и запуск проекта
echo ========================================================================
echo.

set "ROOT_DIR=%~dp0"
cd /d "%ROOT_DIR%"

:: 1. Сборка React Fluent UI (Vite + TypeScript)
echo [1/3] Сборка React интерфейса...
call pnpm --prefix crates\owl-app\ui run build
if %ERRORLEVEL% neq 0 (
    echo [ОШИБКА] Сборка фронтенда завершилась с ошибкой!
    pause
    exit /b %ERRORLEVEL%
)
echo [OK] Фронтенд успешно собран.
echo.

:: 2. Компиляция Rust бинарников (Release)
echo [2/3] Компиляция Release бинарников (owl-app и owl-daemon)...
call cargo build --release -p owl-app -p owl-daemon
if %ERRORLEVEL% neq 0 (
    echo [ОШИБКА] Сборка Cargo завершилась с ошибкой!
    pause
    exit /b %ERRORLEVEL%
)
echo [OK] Бинарники скомпилированы.
echo.

:: 3. Запуск приложения
echo [3/3] Запуск OpenWaveLink...
taskkill /f /im owl-app.exe >nul 2>&1
taskkill /f /im owl-daemon.exe >nul 2>&1

start "" "%ROOT_DIR%target\release\owl-daemon.exe"
timeout /t 1 /nobreak >nul
start "" "%ROOT_DIR%target\release\owl-app.exe"

echo ========================================================================
echo  OpenWaveLink успешно запущен!
echo ========================================================================
