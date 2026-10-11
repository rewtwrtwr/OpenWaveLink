; ========================================================================
; OpenWaveLink Windows 11 Installer Script (Inno Setup 6)
; Architecture: Windows 11 x64 (AMD64)
; Components: owl-daemon, owl-app (Tauri Fluent UI), WDM WaveRT Driver
; Copyright (c) 2026 OpenWaveLink Systems. All Rights Reserved.
; ========================================================================

#define MyAppName "OpenWaveLink"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "OpenWaveLink Systems"
#define MyAppURL "https://github.com/OpenWaveLink/OpenWaveLink"
#define MyAppExeName "owl-app.exe"
#define MyDaemonExeName "owl-daemon.exe"

[Setup]
AppId={{8B1D4F92-56A1-4BE9-9807-C3FA291B8602}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
LicenseFile=..\LICENSE
OutputDir=dist
OutputBaseFilename=OpenWaveLink_v1.0.0_Setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
MinVersion=10.0.22000
SetupIconFile=compiler:SetupModernIcon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "Автозапуск в системном трее при старте Windows 11"; GroupDescription: "Параметры запуска:"

[Files]
; 1. Core Executables (Rust 2024 Release)
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\{#MyDaemonExeName}"; DestDir: "{app}"; Flags: ignoreversion

; 2. Frontend Assets (WebView2 / Fluent UI)
Source: "..\crates\owl-app\ui\dist\*"; DestDir: "{app}\ui"; Flags: ignoreversion recursesubdirs createallsubdirs

; 3. WDM WaveRT Kernel Audio Driver Package
Source: "..\driver\package\OpenWaveLinkAudio.inf"; DestDir: "{app}\driver"; Flags: ignoreversion
Source: "..\driver\package\OpenWaveLinkAudio.sys"; DestDir: "{app}\driver"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} (Консольный демон)"; Filename: "{app}\{#MyDaemonExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; Автозапуск приложения в трее Windows 11
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"" --tray"; Flags: uninsdeletevalue; Tasks: autostart

; Системные оптимизации Windows MMCSS (Multimedia Class Scheduler Service) для Pro Audio
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile"; ValueType: dword; ValueName: "SystemResponsiveness"; ValueData: 0; Flags: createvalueifdoesntexist
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Pro Audio"; ValueType: string; ValueName: "Scheduling Category"; ValueData: "High"; Flags: createvalueifdoesntexist
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Pro Audio"; ValueType: dword; ValueName: "Priority"; ValueData: 24; Flags: createvalueifdoesntexist
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Pro Audio"; ValueType: dword; ValueName: "Clock Rate"; ValueData: 10000; Flags: createvalueifdoesntexist

[Run]
; Установка и регистрация WDM WaveRT драйвера через штатную утилиту Windows pnputil.exe
Filename: "{sys}\pnputil.exe"; Parameters: "/add-driver ""{app}\driver\OpenWaveLinkAudio.inf"" /install"; StatusMsg: "Установка и регистрация открытого WDM WaveRT аудиодрайвера OpenWaveLink в Windows 11..."; Flags: runhidden waituntilterminated

; Запуск приложения после завершения инсталляции
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Корректное удаление драйвера при деинсталляции
Filename: "{sys}\pnputil.exe"; Parameters: "/delete-driver OpenWaveLinkAudio.inf /uninstall /force"; Flags: runhidden waituntilterminated

[Code]
// Функция предварительной проверки совместимости с Windows 11
function InitializeSetup(): Boolean;
var
  Version: TWindowsVersion;
begin
  GetWindowsVersionEx(Version);
  // Проверка номера сборки Windows 11 (22000+)
  if (Version.Major < 10) or ((Version.Major = 10) and (Version.Build < 22000)) then
  begin
    MsgBox('OpenWaveLink оптимизирован строго под Windows 11 (сборка 22000 или выше) для использования аппаратного Process-Loopback API и WaveRT.' + #13#10 +
           'Ваша версия Windows не поддерживается.', mbError, MB_OK);
    Result := False;
    Exit;
  end;
  Result := True;
end;
