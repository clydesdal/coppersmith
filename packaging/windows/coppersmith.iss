; The Windows installer, built by make-installer.ps1 with Inno Setup 6
; (https://jrsoftware.org/isinfo.php). It installs for the one user without
; asking for an administrator, unless they choose everyone; adds Coppersmith
; to the Start menu, opens .cspcb boards with it, and can be taken out again
; from Settings > Apps. Boards and the footprint folder aren't touched by
; the uninstaller.
;
; make-installer.ps1 passes these; they're here so the script also compiles
; on its own from the Inno Setup window.
#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Root
  #define Root "..\.."
#endif

[Setup]
AppId={{0250352C-624A-4719-A659-07209FA54C02}
AppName=Coppersmith
AppVersion={#Version}
AppVerName=Coppersmith {#Version}
AppPublisher=Dal Wheeler
AppPublisherURL=https://github.com/clydesdal/coppersmith
DefaultDirName={autopf}\Coppersmith
DefaultGroupName=Coppersmith
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
SetupIconFile={#Root}\target\coppersmith.ico
UninstallDisplayIcon={app}\coppersmith.exe
UninstallDisplayName=Coppersmith
OutputDir={#Root}\target
OutputBaseFilename=coppersmith-{#Version}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Put Coppersmith on the desktop"; Flags: unchecked
Name: "assoc"; Description: "Open .cspcb boards with Coppersmith"

[Files]
Source: "{#Root}\target\release\coppersmith.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\target\coppersmith.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Coppersmith"; Filename: "{app}\coppersmith.exe"; IconFilename: "{app}\coppersmith.ico"
Name: "{autodesktop}\Coppersmith"; Filename: "{app}\coppersmith.exe"; IconFilename: "{app}\coppersmith.ico"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.cspcb"; ValueType: string; ValueName: ""; ValueData: "Coppersmith.Board"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.cspcb\OpenWithProgids"; ValueType: string; ValueName: "Coppersmith.Board"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Coppersmith.Board"; ValueType: string; ValueName: ""; ValueData: "Coppersmith board"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Coppersmith.Board\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\coppersmith.ico"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Coppersmith.Board\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\coppersmith.exe"" ""%1"""; Tasks: assoc

[Run]
Filename: "{app}\coppersmith.exe"; Description: "Start Coppersmith"; Flags: nowait postinstall skipifsilent
