; Inno Setup script for the Windows installer, built by the package job in ci.yml:
; iscc /DVersion=<build> /DSource=<folder with trenchdot.exe> /DOutput=<folder> tools/installer/trenchdot.iss

#ifndef Version
  #define Version "0.0.0-local"
#endif
#ifndef Source
  #define Source "..\..\target\release"
#endif
#ifndef Output
  #define Output "..\..\dist"
#endif
#define Exe "trenchdot-windows-x86_64.exe"

[Setup]
AppId={{8F1C2B7E-4D3A-4C55-9E62-7A0B1D5E3C91}
AppName=TrenchDot
AppVersion={#Version}
AppPublisher=Paraxdev
AppPublisherURL=https://github.com/Paraxdev/TrenchDot
AppSupportURL=https://github.com/Paraxdev/TrenchDot/issues
AppUpdatesURL=https://github.com/Paraxdev/TrenchDot/releases/tag/beta
; A per user folder, so the editor can replace its own executable when it updates itself.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\TrenchDot
DisableProgramGroupPage=yes
DisableDirPage=auto
UninstallDisplayIcon={app}\{#Exe}
UninstallDisplayName=TrenchDot
LicenseFile=..\..\LICENSE
OutputDir={#Output}
OutputBaseFilename=trenchdot-windows-x86_64-setup
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "tdmassoc"; Description: "Open .tdm maps with TrenchDot"; GroupDescription: "Files:"

[InstallDelete]
; Installs before the executable carried the platform name.
Type: files; Name: "{app}\trenchdot.exe"
Type: files; Name: "{app}\trenchdot-previous.exe"

[Files]
Source: "{#Source}\trenchdot.exe"; DestDir: "{app}"; DestName: "{#Exe}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\TrenchDot"; Filename: "{app}\{#Exe}"
Name: "{autodesktop}\TrenchDot"; Filename: "{app}\{#Exe}"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.tdm\OpenWithProgids"; ValueType: string; ValueName: "TrenchDot.Map"; ValueData: ""; Flags: uninsdeletevalue; Tasks: tdmassoc
Root: HKA; Subkey: "Software\Classes\TrenchDot.Map"; ValueType: string; ValueName: ""; ValueData: "TrenchDot map"; Flags: uninsdeletekey; Tasks: tdmassoc
Root: HKA; Subkey: "Software\Classes\TrenchDot.Map\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#Exe},0"; Tasks: tdmassoc
Root: HKA; Subkey: "Software\Classes\TrenchDot.Map\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#Exe}"" ""%1"""; Tasks: tdmassoc

[Run]
Filename: "{app}\{#Exe}"; Description: "{cm:LaunchProgram,TrenchDot}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What the editor's self update leaves beside it, see crates/td_editor/src/updater.rs.
Type: files; Name: "{app}\trenchdot-windows-x86_64-previous.exe"
Type: files; Name: "{app}\.trenchdot-*"
