; Inno Setup script for Dinosaur — produces a Windows installer (.exe).
;
; Build:
;   1. Install Inno Setup 6+  (https://jrsoftware.org/isinfo.php)
;   2. First build the app + portable bundle:  pwsh scripts\package.ps1
;   3. Compile this script:  iscc installer\Dinosaur.iss
;      (or open it in the Inno Setup Compiler GUI and press F9)
;
; Output:  dist\DinosaurSetup-<version>-win64.exe
;
; Update AppVersion to match Cargo.toml when you cut a release.

#define MyAppName     "Dinosaur"
#define MyAppVersion  "0.1.0"
#define MyAppExeName  "Dinosaur.exe"

[Setup]
AppId={{B6F3A1C2-4E7D-4A9B-9C21-7D3F2E8A1B45}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppName}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=DinosaurSetup-{#MyAppVersion}-win64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per-user install needs no admin rights; use "admin" for all-users.
PrivilegesRequiredOverridesAllowed=dialog
SetupIconFile=..\app\assets\app.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "associatecsv"; Description: "Open .csv / .tsv files with {#MyAppName}"; GroupDescription: "File associations:"; Flags: unchecked

[Files]
; These three files come from the release build (run package.ps1 first).
Source: "..\app\target\release\Dinosaur.exe";        DestDir: "{app}"; Flags: ignoreversion
Source: "..\app\target\release\opengl32.dll";         DestDir: "{app}"; Flags: ignoreversion
Source: "..\app\target\release\libgallium_wgl.dll";   DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}";        Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}";  Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; Optional file associations (only when the task is selected).
Root: HKA; Subkey: "Software\Classes\.csv\OpenWithProgids"; ValueType: string; ValueName: "Dinosaur.csv"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatecsv
Root: HKA; Subkey: "Software\Classes\.tsv\OpenWithProgids"; ValueType: string; ValueName: "Dinosaur.csv"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associatecsv
Root: HKA; Subkey: "Software\Classes\Dinosaur.csv"; ValueType: string; ValueName: ""; ValueData: "Delimited data file"; Flags: uninsdeletekey; Tasks: associatecsv
Root: HKA; Subkey: "Software\Classes\Dinosaur.csv\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associatecsv
Root: HKA; Subkey: "Software\Classes\Dinosaur.csv\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associatecsv

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
