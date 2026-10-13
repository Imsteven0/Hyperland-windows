; Inno Setup script for Caelestia for Windows (built by the Release workflow).
;   iscc /DAppVersion=0.2.0 packaging\installer.iss   ->  dist\caelestia-win-0.2.0-setup.exe
; Per-user install, no admin rights: same folder as scripts\install.ps1.
; Uninstalling keeps your dots (~\.config\caelestia).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppExe "caelestia.exe"

[Setup]
AppId={{02D8EE81-CA2D-4925-9411-D883FBB713A7}
AppName=Caelestia for Windows
AppVersion={#AppVersion}
AppVerName=Caelestia for Windows {#AppVersion}
AppPublisher=Steven Araya Gonzalez
AppPublisherURL=https://github.com/Imsteven0/Hyperland-windows
AppSupportURL=https://github.com/Imsteven0/Hyperland-windows/issues
AppUpdatesURL=https://github.com/Imsteven0/Hyperland-windows/releases
DefaultDirName={localappdata}\Programs\Caelestia
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
; x64compatible: x64 Windows, and Windows on Arm (runs the x64 build emulated).
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputDir=..\dist
OutputBaseFilename=caelestia-win-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\LICENSE
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName=Caelestia for Windows
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[CustomMessages]
en.AddToPath=Add "caelestia" to PATH (to run "caelestia toggle launcher" from a terminal)
es.AddToPath=Añadir "caelestia" al PATH (para usar "caelestia toggle launcher" desde una terminal)
en.LaunchNow=Start Caelestia now
es.LaunchNow=Iniciar Caelestia ahora

[Tasks]
Name: "addtopath"; Description: "{cm:AddToPath}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\fonts\*"; DestDir: "{app}\assets\fonts"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Caelestia"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Comment: "Caelestia shell for Windows"

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Description: "{cm:LaunchNow}"; Flags: nowait postinstall skipifsilent
; In-app updates run the installer silently: start the shell again when done.
Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Flags: nowait; Check: WizardSilent

[UninstallRun]
; Releases the reserved screen space and restores any click-through windows.
Filename: "{app}\{#AppExe}"; Parameters: "quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitShell"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then
    Path := '';
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure RemoveFromPath(Dir: string);
var
  Path: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then
    exit;
  P := Pos(';' + Uppercase(Dir), Uppercase(Path));
  Log('PATH cleanup: looking for ' + Dir + ' in ' + Path + ' -> ' + IntToStr(P));
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir) + 1);
    RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Path);
  end;
end;

{ Ask a running shell to quit before files are replaced (upgrades). }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Exe: string;
  Code: Integer;
begin
  Result := '';
  Exe := ExpandConstant('{app}\{#AppExe}');
  if FileExists(Exe) then
  begin
    Exec(Exe, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
    Sleep(1500);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { Before the files go (the shell was already asked to quit by [UninstallRun]). }
  if CurUninstallStep = usUninstall then
  begin
    { The shell registers itself to start with Windows (windows.autostart). }
    if RegDeleteValue(HKCU, RunKey, 'Caelestia') then
      Log('Removed the autostart entry');
    RemoveFromPath(ExpandConstant('{app}'));
  end;
end;
