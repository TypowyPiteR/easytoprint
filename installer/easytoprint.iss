; EasyToPrint installer script (Inno Setup 6)
; Build: ISCC.exe installer\easytoprint.iss  (from the repo root)

#define MyAppName "EasyToPrint"
#define MyAppVersion "0.2.1"
#define MyAppPublisher "TypowyPiteR"
#define MyAppURL "https://github.com/TypowyPiteR/easytoprint"
#define MyAppExeName "easytoprint.exe"

[Setup]
AppId={{9A5F3C2B-7E41-4B6D-9C0F-2E8A41D5B773}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
OutputDir=..\dist
OutputBaseFilename=EasyToPrint-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequiredOverridesAllowed=dialog
LicenseFile=..\LICENSE

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "pl"; MessagesFile: "compiler:Languages\Polish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\dist\easytoprint.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\lib\pdfium.dll"; DestDir: "{app}\lib"; Flags: ignoreversion
Source: "..\dist\lang\*.json"; DestDir: "{app}\lang"; Flags: ignoreversion
Source: "..\dist\README.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autodesktop}\Easy to Print"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon
Name: "{group}\Easy to Print"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; settings saved in %APPDATA% for Program Files installs
Type: files; Name: "{userappdata}\EasyToPrint\easytoprint.json"
