; ─── Zev Detector — Windows installer ───────────────────────────────────────
;
; Builds Zev-Detector-Setup-<version>.exe: a normal Windows wizard that
; installs both binaries, registers the Start Menu / Desktop shortcuts, and
; offers to open the app at the end.
;
; It deliberately does NOT ask for the server URL, organisation or building.
; The app has its own first-run wizard for that, which can validate what it is
; given and install the service itself — an installer page could only write
; the values blindly. So: the installer installs, the app configures.
;
; Uninstall stops and removes the ZevDetector service first, so no orphaned
; service entry is left pointing at a deleted exe.
;
; Build:
;   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\setup.iss
; (run cargo build --release first)

#define AppName        "Zev Detector"
#define AppVersion     "1.8.0"
#define AppPublisher   "Zev Tabs LLC"
#define AppURL         "https://admin.zevtabs.mn"
#define UiExe          "zev-detector-ui.exe"
#define AgentExe       "zev-detector.exe"
#define ServiceName    "ZevDetector"
#define SrcDir         "..\target\release"

[Setup]
AppId={{8F3A6C42-1B7D-4E59-9C21-7A5E0D3B84F1}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=no
OutputDir=..\dist
OutputBaseFilename=Zev-Detector-Setup-{#AppVersion}
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\{#UiExe}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; The service must be registered under LocalSystem and the files land in
; Program Files, so the whole install needs elevation.
PrivilegesRequired=admin
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "{#SrcDir}\{#UiExe}";   DestDir: "{app}"; Flags: ignoreversion
Source: "{#SrcDir}\{#AgentExe}"; DestDir: "{app}"; Flags: ignoreversion
; Reference copy only — the app writes the real config.toml itself. Shipping
; it as config.toml with onlyifdoesntexist would silently overwrite nothing on
; upgrade but would also hand the service a config pointing at sample paths.
Source: "..\config.sample.toml"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md";          DestDir: "{app}"; Flags: ignoreversion isreadme

[Dirs]
; config.toml, the offline queue, the read offsets and the log live here rather
; than in {app}: the desktop app runs unelevated and cannot write to Program
; Files, while the service runs as LocalSystem. ProgramData is the one place
; both can reach, and users-modify is what lets the app save without elevation.
Name: "{commonappdata}\{#AppName}"; Permissions: users-modify

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#UiExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#UiExe}"; Tasks: desktopicon

[Run]
; Auto-install and start the Windows service immediately with zero-config
Filename: "{app}\{#AgentExe}"; Parameters: "install"; Flags: runhidden waituntilterminated
Filename: "{app}\{#UiExe}"; Description: "Open {#AppName}"; \
  Flags: postinstall nowait skipifsilent

[UninstallRun]
; Stop before delete: a running service holds the exe open and would leave the
; SCM entry behind. Both are best-effort — a service that was never installed
; simply exits non-zero, which must not abort the uninstall.
Filename: "{app}\{#AgentExe}"; Parameters: "stop"; \
  Flags: runhidden waituntilterminated; RunOnceId: "StopZevDetector"
Filename: "{app}\{#AgentExe}"; Parameters: "uninstall"; \
  Flags: runhidden waituntilterminated; RunOnceId: "RemoveZevDetector"

[UninstallDelete]
; Runtime state, which lives in ProgramData. config.toml is handled separately
; in [Code] because it is the one file worth asking about before deleting.
Type: files; Name: "{commonappdata}\{#AppName}\zev-detector.log"
Type: files; Name: "{commonappdata}\{#AppName}\state.json"
Type: files; Name: "{commonappdata}\{#AppName}\queue.json"
Type: dirifempty; Name: "{commonappdata}\{#AppName}"
; Pre-ProgramData builds wrote these into {app}; without them {app} is never
; empty and survives the uninstall.
Type: files; Name: "{app}\zev-detector.log"
Type: files; Name: "{app}\state.json"
Type: files; Name: "{app}\queue.json"
Type: dirifempty; Name: "{app}"

[Code]
{ An upgrade over a running service can't replace the exe while it is loaded,
  so stop it before copying files and let the [Run] section bring it back. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  AgentPath: String;
begin
  Result := '';
  AgentPath := ExpandConstant('{app}\{#AgentExe}');
  if FileExists(AgentPath) then
  begin
    Exec(AgentPath, 'stop', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    { Give the SCM a moment to actually release the binary. }
    Sleep(1500);
  end;
end;

{ Leaving config.toml behind on uninstall would silently reuse a stale server
  URL if the product is reinstalled later, so ask rather than assume. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ConfigPath: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    ConfigPath := ExpandConstant('{commonappdata}\{#AppName}\config.toml');
    if FileExists(ConfigPath) then
      if MsgBox('Remove the Zev Detector configuration (config.toml) as well?',
                mbConfirmation, MB_YESNO) = IDYES then
        DeleteFile(ConfigPath);
  end;
end;
