; ─── Zev Detector — Windows installer ───────────────────────────────────────
;
; Builds Zev-Detector-Setup-<version>.exe: a normal Windows wizard that
; installs both binaries, registers the Start Menu / Desktop shortcuts, and
; offers to open the app at the end.
;
; Two wizard pages configure the basics so nobody has to open the app:
;   1. PC name (prefilled from an existing config.toml, else the Windows name;
;      silent installs can pass /NAME="...")
;   2. Camera picker — runs `zev-detector.exe discover` (Dahua, Hikvision SADP,
;      ONVIF, subnet scan for ZKTeco / Tiger Wong) and lists what it finds;
;      an IP can also be typed in by hand.
; Both are written by `zev-detector.exe setup` before the service starts.
;   3. Parking service (optional): Dahua `dahua-service` or Hikvision
;      `hikvision-service`, bundled from D:\zevtabs. Token, building ID and
;      camera roles are asked here; `zev-detector.exe parking-setup` writes its
;      config.toml, moves an already-registered copy (e.g. from D:\zevtabs) into
;      {app}\parking\<kind>, registers and starts it.
; Server URL / organisation still come from the server via the heartbeat.
;
; Uninstall stops and removes the ZevDetector service first, so no orphaned
; service entry is left pointing at a deleted exe.
;
; Build:
;   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\setup.iss
; (run cargo build --release first)

#define AppName        "Zev Detector"
#define AppVersion     "1.10.0"
#define AppPublisher   "Zev Tabs LLC"
#define AppURL         "https://admin.zevtabs.mn"
#define UiExe          "zev-detector-ui.exe"
#define AgentExe       "zev-detector.exe"
#define ServiceName    "ZevDetector"
#define SrcDir         "..\target\release"
#define DahuaSrc       "D:\zevtabs\dahuarust\dahuaExeRust"
#define HikSrc         "D:\zevtabs\hikRustExe"

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
; Second copy the wizard extracts to {tmp} to scan for cameras before anything
; is installed (the files above are not on disk yet at that point).
Source: "{#SrcDir}\{#AgentExe}"; DestName: "zev-discover.exe"; Flags: dontcopy
; Reference copy only — the app writes the real config.toml itself. Shipping
; it as config.toml with onlyifdoesntexist would silently overwrite nothing on
; upgrade but would also hand the service a config pointing at sample paths.
Source: "..\config.sample.toml"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md";          DestDir: "{app}"; Flags: ignoreversion isreadme
; Parking services — only the one picked on the "Зогсоолын үйлчилгээ" page.
; config.toml is never shipped: parking-setup writes it from the wizard answers.
Source: "{#DahuaSrc}\target\release\dahua-service.exe"; DestDir: "{app}\parking\dahua"; Flags: ignoreversion; Check: ParkIs('dahua')
Source: "{#DahuaSrc}\dlls\*.dll"; DestDir: "{app}\parking\dahua"; Flags: ignoreversion; Check: ParkIs('dahua')
Source: "{#HikSrc}\target\release\hikvision-service.exe"; DestDir: "{app}\parking\hikvision"; Flags: ignoreversion; Check: ParkIs('hikvision')
Source: "{#HikSrc}\dlls\*.dll"; DestDir: "{app}\parking\hikvision"; Flags: ignoreversion; Check: ParkIs('hikvision')

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
; Name/cameras, the parking service and the ZevDetector service itself are set
; up in [Code] CurStepChanged(ssPostInstall), in that order.
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
Filename: "{sys}\sc.exe"; Parameters: "stop DahuaParkingService"; Check: ParkExeInApp('dahua'); \
  Flags: runhidden waituntilterminated; RunOnceId: "StopDahuaParking"
Filename: "{app}\parking\dahua\dahua-service.exe"; Parameters: "uninstall"; Check: ParkExeInApp('dahua'); \
  Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "RemoveDahuaParking"
Filename: "{sys}\sc.exe"; Parameters: "stop zevHikvision"; Check: ParkExeInApp('hikvision'); \
  Flags: runhidden waituntilterminated; RunOnceId: "StopHikParking"
Filename: "{app}\parking\hikvision\hikvision-service.exe"; Parameters: "uninstall"; Check: ParkExeInApp('hikvision'); \
  Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "RemoveHikParking"

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
; Parking service runtime files (config.toml is asked about in [Code])
Type: files; Name: "{app}\parking\dahua\service.log"
Type: files; Name: "{app}\parking\dahua\parking-setup.log"
Type: files; Name: "{app}\parking\dahua\config.toml.bak"
Type: files; Name: "{app}\parking\hikvision\hikvision-service.log"
Type: files; Name: "{app}\parking\hikvision\error.log"
Type: files; Name: "{app}\parking\hikvision\parking-setup.log"
Type: files; Name: "{app}\parking\hikvision\config.toml.bak"
Type: dirifempty; Name: "{app}\parking\dahua"
Type: dirifempty; Name: "{app}\parking\hikvision"
Type: dirifempty; Name: "{app}\parking"
Type: dirifempty; Name: "{app}"

[Code]
function ParkService(I: Integer): String;
begin
  case I of
    1: Result := 'DahuaParkingService';
    2: Result := 'zevHikvision';
  else
    Result := '';
  end;
end;

{ Folder of the exe a service is registered with, '' if not installed }
function ServiceExeDir(Svc: String): String;
var
  P: String;
  E: Integer;
begin
  Result := '';
  if Svc = '' then exit;
  if not RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Services\' + Svc, 'ImagePath', P) then exit;
  P := Trim(P);
  if (Length(P) > 0) and (P[1] = '"') then
  begin
    Delete(P, 1, 1);
    if Pos('"', P) > 0 then P := Copy(P, 1, Pos('"', P) - 1);
  end
  else
  begin
    E := Pos('.exe', LowerCase(P));
    if E > 0 then P := Copy(P, 1, E + 3);
  end;
  Result := ExtractFileDir(P);
end;

{ N-th (0-based) '|' part of a camera spec ip|port|vendor|model }
function SpecPart(S: String; N: Integer): String;
var
  P: Integer;
begin
  while N > 0 do
  begin
    P := Pos('|', S);
    if P = 0 then
    begin
      Result := '';
      exit;
    end;
    S := Copy(S, P + 1, MaxInt);
    N := N - 1;
  end;
  P := Pos('|', S);
  if P > 0 then S := Copy(S, 1, P - 1);
  Result := S;
end;

{ Uninstall Check: this parking service was installed by us }
function ParkExeInApp(Key: String): Boolean;
begin
  if Key = 'dahua' then
    Result := FileExists(ExpandConstant('{app}\parking\dahua\dahua-service.exe'))
  else
    Result := FileExists(ExpandConstant('{app}\parking\hikvision\hikvision-service.exe'));
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
    if FileExists(ConfigPath) or
       FileExists(ExpandConstant('{app}\parking\dahua\config.toml')) or
       FileExists(ExpandConstant('{app}\parking\hikvision\config.toml')) then
      if MsgBox('Тохиргооны файлуудыг (config.toml — token, камерын нууц үг) мөн устгах уу?',
                mbConfirmation, MB_YESNO) = IDYES then
      begin
        DeleteFile(ConfigPath);
        DeleteFile(ExpandConstant('{app}\parking\dahua\config.toml'));
        DeleteFile(ExpandConstant('{app}\parking\hikvision\config.toml'));
        RemoveDir(ExpandConstant('{app}\parking\dahua'));
        RemoveDir(ExpandConstant('{app}\parking\hikvision'));
        RemoveDir(ExpandConstant('{app}\parking'));
        RemoveDir(ExpandConstant('{app}'));
      end;
  end;
end;

{ ─── PC name + camera picker pages ─────────────────────────────────────── }
var
  NamePage: TInputQueryWizardPage;
  CamPage: TWizardPage;
  CamList: TNewCheckListBox;
  CamStatus: TNewStaticText;
  ScanBtn, AddBtn: TNewButton;
  ManualEdit: TNewEdit;
  CamSpecs: TArrayOfString;   { ip|port|vendor|model, same index as CamList }
  Scanned: Boolean;

function ConfigFile(): String;
begin
  Result := ExpandConstant('{commonappdata}\{#AppName}\config.toml');
end;

{ name = "..." under [agent] in an existing config.toml (upgrade keeps the name) }
function ExistingAgentName(): String;
var
  Lines: TArrayOfString;
  I, P: Integer;
  InAgent: Boolean;
  L, V: String;
begin
  Result := '';
  if not LoadStringsFromFile(ConfigFile(), Lines) then exit;
  InAgent := False;
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    L := Trim(Lines[I]);
    if (Length(L) > 0) and (L[1] = '[') then
      InAgent := (L = '[agent]')
    else if InAgent and (Copy(L, 1, 4) = 'name') then
    begin
      P := Pos('=', L);
      if P = 0 then continue;
      if Trim(Copy(L, 5, P - 5)) <> '' then continue; { e.g. name_xyz }
      V := Trim(Copy(L, P + 1, MaxInt));
      if (Length(V) > 0) and (V[1] = '"') then
      begin
        Delete(V, 1, 1);
        if Pos('"', V) > 0 then V := Copy(V, 1, Pos('"', V) - 1);
      end;
      Result := Trim(V);
      exit;
    end;
  end;
end;

{ N-th (0-based) tab-separated field }
function Field(S: String; N: Integer): String;
var
  P: Integer;
begin
  while N > 0 do
  begin
    P := Pos(#9, S);
    if P = 0 then
    begin
      Result := '';
      exit;
    end;
    S := Copy(S, P + 1, MaxInt);
    N := N - 1;
  end;
  P := Pos(#9, S);
  if P > 0 then S := Copy(S, 1, P - 1);
  Result := Trim(S);
end;

function CleanArg(S: String): String;
begin
  StringChangeEx(S, '"', '', True);
  StringChangeEx(S, '|', ' ', True);
  S := Trim(S);
  while (Length(S) > 0) and (S[Length(S)] = '\') do
    S := Copy(S, 1, Length(S) - 1);
  Result := S;
end;

function IsIPv4(S: String): Boolean;
var
  I, Parts, Num, Digits: Integer;
  C: Char;
begin
  Result := False;
  Parts := 0;
  Num := 0;
  Digits := 0;
  S := S + '.';
  for I := 1 to Length(S) do
  begin
    C := S[I];
    if (C >= '0') and (C <= '9') then
    begin
      Num := Num * 10 + (Ord(C) - Ord('0'));
      Digits := Digits + 1;
      if (Num > 255) or (Digits > 3) then exit;
    end
    else if C = '.' then
    begin
      if Digits = 0 then exit;
      Parts := Parts + 1;
      Num := 0;
      Digits := 0;
    end
    else
      exit;
  end;
  Result := (Parts = 4);
end;

procedure AddCamera(Ip, Port, Vendor, Model, Info: String; Checked: Boolean);
var
  I: Integer;
  Caption: String;
begin
  for I := 0 to GetArrayLength(CamSpecs) - 1 do
    if Copy(CamSpecs[I], 1, Length(Ip) + 1) = Ip + '|' then exit;
  if Port = '' then Port := '80';
  Caption := Ip + '    ' + Vendor;
  if Model <> '' then Caption := Caption + ' - ' + Model;
  CamList.AddCheckBox(Caption, Info, 0, Checked, True, False, False, nil);
  SetArrayLength(CamSpecs, GetArrayLength(CamSpecs) + 1);
  CamSpecs[GetArrayLength(CamSpecs) - 1] :=
    CleanArg(Ip) + '|' + CleanArg(Port) + '|' + CleanArg(Vendor) + '|' + CleanArg(Model);
end;

procedure ScanCameras();
var
  Exe, OutF, L, Vendor, Info: String;
  Lines: TArrayOfString;
  Code, I, Found: Integer;
begin
  ScanBtn.Enabled := False;
  WizardForm.NextButton.Enabled := False;
  CamStatus.Caption := 'Сүлжээнээс камер хайж байна... (10 хүртэл секунд)';
  WizardForm.Refresh;

  Found := 0;
  try
    ExtractTemporaryFile('zev-discover.exe');
    Exe := ExpandConstant('{tmp}\zev-discover.exe');
    OutF := ExpandConstant('{tmp}\cameras.tsv');
    DeleteFile(OutF);
    if Exec(Exe, 'discover --tsv --out "' + OutF + '"', '', SW_HIDE, ewWaitUntilTerminated, Code)
       and LoadStringsFromFile(OutF, Lines) then
      for I := 0 to GetArrayLength(Lines) - 1 do
      begin
        L := Lines[I];
        if not IsIPv4(Field(L, 0)) then continue;
        Vendor := Field(L, 1);
        Info := 'порт ' + Field(L, 5);
        if Field(L, 3) <> '' then Info := Field(L, 3) + '   ' + Info;
        { Unknown = only RTSP seen (could be an NVR) - listed, but not pre-ticked }
        AddCamera(Field(L, 0), Field(L, 4), Vendor, Field(L, 2), Info, Vendor <> 'Unknown');
        Found := Found + 1;
      end;
  except
    Log('Camera scan failed: ' + GetExceptionMessage);
  end;

  Scanned := True;
  if Found = 0 then
    CamStatus.Caption := 'Камер олдсонгүй. IP-г гараар нэмж болно, эсвэл алгасаад үргэлжлүүлнэ үү.'
  else
    CamStatus.Caption := IntToStr(Found) + ' төхөөрөмж олдлоо. Энэ компьютерт холбогдсон камеруудыг сонгоно уу.';
  ScanBtn.Enabled := True;
  WizardForm.NextButton.Enabled := True;
end;

procedure ScanBtnClick(Sender: TObject);
begin
  ScanCameras();
end;

procedure AddBtnClick(Sender: TObject);
var
  Ip: String;
begin
  Ip := Trim(ManualEdit.Text);
  if not IsIPv4(Ip) then
  begin
    MsgBox('IP хаяг буруу байна (жишээ нь 192.168.1.108).', mbError, MB_OK);
    exit;
  end;
  AddCamera(Ip, '80', 'Гараар', '', 'гараар нэмсэн', True);
  ManualEdit.Text := '';
end;


{ ─── Bundled parking service (Dahua / Hikvision) ─────────────────────────── }
var
  ParkPage, RolePage: TWizardPage;
  ParkKind: TNewComboBox;          { 0 = none, 1 = Dahua, 2 = Hikvision }
  KeepBox: TNewCheckBox;
  ParkInfo: TNewStaticText;
  TokenEdit, BarilgaEdit, OrgEdit, PassEdit: TNewEdit;
  RoleCombos: array of TNewComboBox;
  RoleLabels: array of TNewStaticText;
  RoleIdx: array of Integer;       { CamSpecs index for each role row }
  ParkDefaulted: Boolean;
  ParkOk: Boolean;
  ForeignPark: String;          { parking services already running from elsewhere }

function KindKey(I: Integer): String;
begin
  case I of
    1: Result := 'dahua';
    2: Result := 'hikvision';
  else
    Result := '';
  end;
end;

function ParkingDir(Key: String): String;
begin
  Result := AddBackslash(WizardDirValue) + 'parking\' + Key;
end;

function ParkingWanted(): Boolean;
begin
  Result := (ParkKind <> nil) and (ParkKind.ItemIndex > 0);
end;

{ [Files] Check: only copy the service the user picked }
function ParkIs(Key: String): Boolean;
begin
  Result := ParkingWanted() and (KindKey(ParkKind.ItemIndex) = Key);
end;

{ Existing config for this kind: the new location first, else where the
  currently registered service lives (e.g. D:\zevtabs\dahuarust\...) }
function ExistingParkingConfig(I: Integer): String;
var
  D: String;
begin
  Result := '';
  if I = 0 then exit;
  if FileExists(ParkingDir(KindKey(I)) + '\config.toml') then
  begin
    Result := ParkingDir(KindKey(I)) + '\config.toml';
    exit;
  end;
  D := ServiceExeDir(ParkService(I));
  if (D <> '') and FileExists(AddBackslash(D) + 'config.toml') then
    Result := AddBackslash(D) + 'config.toml';
end;

{ value of `key = "..."` (first match, any table) }
function TomlValue(Path, Key: String): String;
var
  Lines: TArrayOfString;
  I, P: Integer;
  L, K, V: String;
begin
  Result := '';
  if not LoadStringsFromFile(Path, Lines) then exit;
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    L := Trim(Lines[I]);
    P := Pos('=', L);
    if (P = 0) or (Copy(L, 1, 1) = '#') then continue;
    K := Trim(Copy(L, 1, P - 1));
    if K <> Key then continue;
    V := Trim(Copy(L, P + 1, MaxInt));
    if (Length(V) > 0) and (V[1] = '"') then
    begin
      Delete(V, 1, 1);
      if Pos('"', V) > 0 then V := Copy(V, 1, Pos('"', V) - 1);
    end;
    Result := V;
    exit;
  end;
end;

function InAppDir(D: String): Boolean;
begin
  Result := (D <> '') and
    (Pos(LowerCase(AddBackslash(WizardDirValue)), LowerCase(AddBackslash(D))) = 1);
end;

{ A parking service registered from a folder that is NOT ours (e.g. the site's
  own D:\zevtabs\... install, or ZK's ParkingService). The installer leaves
  these alone unless the user explicitly chooses to replace one. }
function ForeignDir(Svc: String): String;
var
  D: String;
begin
  Result := '';
  D := ServiceExeDir(Svc);
  if (D <> '') and not InAppDir(D) then Result := D;
end;

function ParkingName(I: Integer): String;
begin
  case I of
    0: Result := 'DahuaParkingService';
    1: Result := 'zevdahua';           { older Dahua installs }
    2: Result := 'zevHikvision';
    3: Result := 'hikvision-service';
    4: Result := 'zevhikvision';
    5: Result := 'ParkingService';     { ZK (zkexerust) }
  else
    Result := '';
  end;
end;

{ True when a parking service OTHER than `Keep` is registered from outside the install folder.
  Two parking services on one PC fight over API port 5000 and the cameras. }
function OtherParkingRuns(Keep: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 0 to 5 do
    if (ParkingName(I) <> Keep) and (ForeignDir(ParkingName(I)) <> '') then
    begin
      Result := True;
      exit;
    end;
end;

function ForeignParkingServices(): String;
var
  I: Integer;
  D: String;
begin
  Result := '';
  for I := 0 to 5 do
  begin
    D := ForeignDir(ParkingName(I));
    if D <> '' then
    begin
      if Result <> '' then Result := Result + ', ';
      Result := Result + ParkingName(I) + ' (' + D + ')';
    end;
  end;
end;

procedure UpdateParkFields();
var
  Cfg: String;
  Has, Edit: Boolean;
begin
  Cfg := ExistingParkingConfig(ParkKind.ItemIndex);
  Has := Cfg <> '';
  KeepBox.Visible := ParkingWanted() and Has;
  if not Has then KeepBox.Checked := False;
  Edit := ParkingWanted() and not KeepBox.Checked;
  TokenEdit.Enabled := Edit;
  BarilgaEdit.Enabled := Edit;
  OrgEdit.Enabled := Edit;
  PassEdit.Enabled := Edit;

  if (not ParkingWanted()) and (ForeignPark <> '') then
    ParkInfo.Caption := 'Энэ компьютерт аль хэдийн ажиллаж байна: ' + ForeignPark +
                        ' — хөндөхгүй, зөвхөн Zev Detector суулгана.'
  else if not ParkingWanted() then
    ParkInfo.Caption := 'Зөвхөн Zev Detector суулгана. Зогсоолын үйлчилгээ энэ компьютерт байхгүй.'
  else if Has then
    ParkInfo.Caption := 'Одоогийн тохиргоо олдлоо: ' + Cfg
  else
    ParkInfo.Caption := 'Шинэ суулгалт — token болон барилгын ID-г оруулна уу.';

  { Prefill from the existing config so "untick keep" starts from real values }
  if Has and (TokenEdit.Text = '') then
  begin
    TokenEdit.Text := TomlValue(Cfg, 'token');
    BarilgaEdit.Text := TomlValue(Cfg, 'barilgiinId');
    if BarilgaEdit.Text = '' then BarilgaEdit.Text := TomlValue(Cfg, 'barilgiin_id');
    OrgEdit.Text := TomlValue(Cfg, 'org_name');
    PassEdit.Text := TomlValue(Cfg, 'password');
  end;
end;

procedure ParkKindChange(Sender: TObject);
begin
  TokenEdit.Text := '';
  KeepBox.Checked := True;
  UpdateParkFields();
end;

procedure KeepBoxClick(Sender: TObject);
begin
  UpdateParkFields();
end;

function AddField(Page: TWizardPage; Caption: String; Top: Integer; Secret: Boolean): TNewEdit;
var
  L: TNewStaticText;
begin
  L := TNewStaticText.Create(Page);
  L.Parent := Page.Surface;
  L.Left := 0;
  L.Top := Top + ScaleY(3);
  L.Caption := Caption;
  Result := TNewEdit.Create(Page);
  Result.Parent := Page.Surface;
  Result.Left := ScaleX(130);
  Result.Top := Top;
  Result.Width := Page.SurfaceWidth - ScaleX(130);
  if Secret then Result.PasswordChar := '*';
end;

procedure CreateParkingPages(AfterID: Integer);
var
  L: TNewStaticText;
begin
  ParkPage := CreateCustomPage(AfterID,
    'Зогсоолын үйлчилгээ',
    'Камераас машины дугаар хүлээн авч turees.zevtabs.mn руу илгээх үйлчилгээ.');

  L := TNewStaticText.Create(ParkPage);
  L.Parent := ParkPage.Surface;
  L.Left := 0;
  L.Top := ScaleY(3);
  L.Caption := 'Төрөл:';

  ParkKind := TNewComboBox.Create(ParkPage);
  ParkKind.Parent := ParkPage.Surface;
  ParkKind.Style := csDropDownList;
  ParkKind.Left := ScaleX(130);
  ParkKind.Top := 0;
  ParkKind.Width := ParkPage.SurfaceWidth - ScaleX(130);
  ParkKind.Items.Add('Суулгахгүй');
  ParkKind.Items.Add('Dahua  (DahuaParkingService)');
  ParkKind.Items.Add('Hikvision  (zevHikvision)');
  ParkKind.ItemIndex := 0;
  ParkKind.OnChange := @ParkKindChange;

  ParkInfo := TNewStaticText.Create(ParkPage);
  ParkInfo.Parent := ParkPage.Surface;
  ParkInfo.Left := 0;
  ParkInfo.Top := ScaleY(30);
  ParkInfo.AutoSize := False;
  ParkInfo.WordWrap := True;
  ParkInfo.Width := ParkPage.SurfaceWidth;
  ParkInfo.Height := ScaleY(28);

  KeepBox := TNewCheckBox.Create(ParkPage);
  KeepBox.Parent := ParkPage.Surface;
  KeepBox.Left := 0;
  KeepBox.Top := ScaleY(60);
  KeepBox.Width := ParkPage.SurfaceWidth;
  KeepBox.Caption := 'Одоогийн тохиргоог хэвээр үлдээх (token, камер, нууц үг)';
  KeepBox.Checked := True;
  KeepBox.OnClick := @KeepBoxClick;

  TokenEdit := AddField(ParkPage, 'Token:', ScaleY(88), True);
  BarilgaEdit := AddField(ParkPage, 'Барилгын ID:', ScaleY(116), False);
  OrgEdit := AddField(ParkPage, 'Байгууллагын нэр:', ScaleY(144), False);
  PassEdit := AddField(ParkPage, 'Камерын нууц үг:', ScaleY(172), True);

  RolePage := CreateCustomPage(ParkPage.ID,
    'Камерын үүрэг',
    'Камер бүр орох эсвэл гарах хаалганых эсэхийг сонгоно уу.');

  ParkDefaulted := False;
end;

{ At start: pick the parking service already registered on this PC, keeping
  its settings. A silent install only does this for a service that already
  lives in the install folder — it never silently moves one from elsewhere. }
procedure InitParkFromService();
var
  I: Integer;
  D: String;
begin
  ForeignPark := ForeignParkingServices();
  for I := 1 to 2 do
  begin
    D := ServiceExeDir(ParkService(I));
    { Only an upgrade of a service this installer put in the install folder }
    if not InAppDir(D) then continue;
    ParkKind.ItemIndex := I;
    break;
  end;
  { Something already runs here (ours or the site's own): don't guess from cameras }
  if (ParkKind.ItemIndex > 0) or (ForeignPark <> '') then ParkDefaulted := True;
  KeepBox.Checked := True;
  UpdateParkFields();
end;

{ First visit of the page with nothing picked yet: use the ticked cameras' brand. }
procedure DefaultParkKindFromCameras();
var
  I, Dahua, Hik: Integer;
  V: String;
begin
  if ParkDefaulted then
  begin
    UpdateParkFields();
    exit;
  end;
  ParkDefaulted := True;
  Dahua := 0;
  Hik := 0;
  for I := 0 to GetArrayLength(CamSpecs) - 1 do
    if CamList.Checked[I] then
    begin
      V := SpecPart(CamSpecs[I], 2);
      if V = 'Dahua' then Dahua := Dahua + 1;
      if V = 'Hikvision' then Hik := Hik + 1;
    end;
  if Hik > Dahua then ParkKind.ItemIndex := 2
  else if Dahua > 0 then ParkKind.ItemIndex := 1;
  KeepBox.Checked := True;
  UpdateParkFields();
end;

procedure BuildRoleRows();
var
  I, N, Top: Integer;
begin
  for I := 0 to GetArrayLength(RoleCombos) - 1 do
  begin
    RoleCombos[I].Free;
    RoleLabels[I].Free;
  end;
  SetArrayLength(RoleCombos, 0);
  SetArrayLength(RoleLabels, 0);
  SetArrayLength(RoleIdx, 0);

  N := 0;
  for I := 0 to GetArrayLength(CamSpecs) - 1 do
  begin
    if not CamList.Checked[I] then continue;
    if N >= 7 then break;
    Top := N * ScaleY(30);
    SetArrayLength(RoleCombos, N + 1);
    SetArrayLength(RoleLabels, N + 1);
    SetArrayLength(RoleIdx, N + 1);
    RoleIdx[N] := I;

    RoleLabels[N] := TNewStaticText.Create(RolePage);
    RoleLabels[N].Parent := RolePage.Surface;
    RoleLabels[N].Left := 0;
    RoleLabels[N].Top := Top + ScaleY(3);
    RoleLabels[N].Caption := SpecPart(CamSpecs[I], 0) + '   ' + SpecPart(CamSpecs[I], 2);

    RoleCombos[N] := TNewComboBox.Create(RolePage);
    RoleCombos[N].Parent := RolePage.Surface;
    RoleCombos[N].Style := csDropDownList;
    RoleCombos[N].Left := ScaleX(230);
    RoleCombos[N].Top := Top;
    RoleCombos[N].Width := ScaleX(150);
    RoleCombos[N].Items.Add('Орох (entrance)');
    RoleCombos[N].Items.Add('Гарах (exit)');
    { 1st camera = entrance, 2nd = exit, then alternate }
    RoleCombos[N].ItemIndex := N mod 2;
    N := N + 1;
  end;
end;

function CheckedCameraCount(): Integer;
var
  I: Integer;
begin
  Result := 0;
  for I := 0 to GetArrayLength(CamSpecs) - 1 do
    if CamList.Checked[I] then Result := Result + 1;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if (RolePage <> nil) and (PageID = RolePage.ID) then
    Result := (not ParkingWanted()) or KeepBox.Checked or (CheckedCameraCount() = 0);
end;

function BrandKey(Vendor: String): String;
begin
  if Vendor = 'Dahua' then Result := 'dahua'
  else if Vendor = 'Hikvision' then Result := 'hikvision'
  else Result := 'auto';
end;

{ Runs `parking-setup` and reports. Answers go through a temp file (deleted by
  the agent right after reading) so the token never appears on a command
  line or in the setup log. }
procedure RunParkingSetup();
var
  Lines, Report: TArrayOfString;
  I, N, Code: Integer;
  Answers, Key, Gate, Msg: String;
begin
  ParkOk := True;
  if not ParkingWanted() then exit;
  Key := KindKey(ParkKind.ItemIndex);
  Answers := ExpandConstant('{tmp}\parking-answers.txt');

  SetArrayLength(Lines, 5);
  Lines[0] := 'token=' + Trim(TokenEdit.Text);
  Lines[1] := 'barilgiinId=' + Trim(BarilgaEdit.Text);
  Lines[2] := 'org_name=' + Trim(OrgEdit.Text);
  Lines[3] := 'password=' + PassEdit.Text;
  if KeepBox.Checked then Lines[4] := 'keep=1' else Lines[4] := 'keep=0';
  N := 5;
  if not KeepBox.Checked then
    for I := 0 to GetArrayLength(RoleIdx) - 1 do
    begin
      if RoleCombos[I].ItemIndex = 1 then Gate := 'exit' else Gate := 'entrance';
      SetArrayLength(Lines, N + 1);
      Lines[N] := 'camera=' + SpecPart(CamSpecs[RoleIdx[I]], 0) + '|' + Gate + '|' +
                  BrandKey(SpecPart(CamSpecs[RoleIdx[I]], 2));
      N := N + 1;
    end;
  SaveStringsToUTF8File(Answers, Lines, False);

  WizardForm.StatusLabel.Caption := 'Зогсоолын үйлчилгээг тохируулж байна...';
  Exec(ExpandConstant('{app}\{#AgentExe}'),
       'parking-setup --kind ' + Key + ' --dir "' + ParkingDir(Key) + '" --answers "' + Answers + '"',
       '', SW_HIDE, ewWaitUntilTerminated, Code);
  DeleteFile(Answers);

  Msg := '';
  if LoadStringsFromFile(ParkingDir(Key) + '\parking-setup.log', Report) then
    for I := 1 to GetArrayLength(Report) - 1 do
      Msg := Msg + Report[I] + #13#10;
  Log('parking-setup exit ' + IntToStr(Code) + ': ' + Msg);
  if Code <> 0 then
  begin
    ParkOk := False;
    if not WizardSilent then
      MsgBox('Зогсоолын үйлчилгээг тохируулж чадсангүй:' + #13#10#13#10 + Msg + #13#10 +
             'Zev Detector суусан. Тохиргоог ' + ParkingDir(Key) + '\config.toml дээр засаад үйлчилгээг эхлүүлнэ үү.',
             mbError, MB_OK);
  end;
end;


procedure InitializeWizard();
var
  Def: String;
  Lbl: TNewStaticText;
begin
  NamePage := CreateInputQueryPage(wpSelectDir,
    'Компьютерийн нэр',
    'Админ системд (admin.zevtabs.mn) энэ нэрээр харагдана.',
    'Зогсоолын нэрээр өгвөл танихад амар (жишээ нь: Omnitriks, MEMAR-Гарц).');
  NamePage.Add('Нэр:', False);
  Def := ExpandConstant('{param:NAME|}');
  if Def = '' then Def := ExistingAgentName();
  if Def = '' then Def := GetComputerNameString();
  NamePage.Values[0] := Def;

  CamPage := CreateCustomPage(NamePage.ID,
    'Камер сонгох',
    'Сонгосон камер сүлжээнээс тасрахад админд мэдэгдэнэ.');

  CamStatus := TNewStaticText.Create(CamPage);
  CamStatus.Parent := CamPage.Surface;
  CamStatus.Left := 0;
  CamStatus.Top := 0;
  CamStatus.AutoSize := False;
  CamStatus.WordWrap := True;
  CamStatus.Width := CamPage.SurfaceWidth - ScaleX(100);
  CamStatus.Height := ScaleY(30);
  CamStatus.Caption := 'Dahua, Hikvision, ZKTeco, Tiger Wong болон ONVIF камеруудыг хайна.';

  ScanBtn := TNewButton.Create(CamPage);
  ScanBtn.Parent := CamPage.Surface;
  ScanBtn.Width := ScaleX(90);
  ScanBtn.Height := ScaleY(25);
  ScanBtn.Left := CamPage.SurfaceWidth - ScanBtn.Width;
  ScanBtn.Top := 0;
  ScanBtn.Caption := 'Дахин хайх';
  ScanBtn.OnClick := @ScanBtnClick;

  CamList := TNewCheckListBox.Create(CamPage);
  CamList.Parent := CamPage.Surface;
  CamList.Left := 0;
  CamList.Top := ScaleY(36);
  CamList.Width := CamPage.SurfaceWidth;
  CamList.Height := CamPage.SurfaceHeight - ScaleY(36 + 34);

  Lbl := TNewStaticText.Create(CamPage);
  Lbl.Parent := CamPage.Surface;
  Lbl.Left := 0;
  Lbl.Top := CamPage.SurfaceHeight - ScaleY(23);
  Lbl.Caption := 'IP гараар:';

  ManualEdit := TNewEdit.Create(CamPage);
  ManualEdit.Parent := CamPage.Surface;
  ManualEdit.Left := ScaleX(70);
  ManualEdit.Top := CamPage.SurfaceHeight - ScaleY(26);
  ManualEdit.Width := ScaleX(140);

  AddBtn := TNewButton.Create(CamPage);
  AddBtn.Parent := CamPage.Surface;
  AddBtn.Left := ManualEdit.Left + ManualEdit.Width + ScaleX(8);
  AddBtn.Top := CamPage.SurfaceHeight - ScaleY(27);
  AddBtn.Width := ScaleX(75);
  AddBtn.Height := ScaleY(25);
  AddBtn.Caption := 'Нэмэх';
  AddBtn.OnClick := @AddBtnClick;

  Scanned := False;

  CreateParkingPages(CamPage.ID);
  InitParkFromService();
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = CamPage.ID) and not Scanned then
    ScanCameras();
  if CurPageID = ParkPage.ID then
    DefaultParkKindFromCameras();
  if CurPageID = RolePage.ID then
    BuildRoleRows();
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = NamePage.ID) and (CleanArg(NamePage.Values[0]) = '') then
  begin
    MsgBox('Компьютерийн нэрийг оруулна уу.', mbError, MB_OK);
    Result := False;
  end;
  if (CurPageID = ParkPage.ID) and ParkingWanted() and (ForeignPark <> '') then
  begin
    if OtherParkingRuns(ParkService(ParkKind.ItemIndex)) then
    begin
      { Two parking services on one PC fight over API port 5000 and the cameras }
      MsgBox('Энэ компьютерт өөр зогсоолын үйлчилгээ ажиллаж байна:' + #13#10 + ForeignPark + #13#10#13#10 +
             'Хоёр үйлчилгээ зэрэг ажиллаж болохгүй (порт 5000, камер). "Суулгахгүй"-г сонгоно уу, ' +
             'эсвэл эхлээд тэр үйлчилгээг устгана уу.', mbError, MB_OK);
      Result := False;
      exit;
    end;
    if MsgBox('Энэ компьютерт ' + ParkService(ParkKind.ItemIndex) + ' аль хэдийн ажиллаж байна:' + #13#10 +
              ForeignDir(ParkService(ParkKind.ItemIndex)) + #13#10#13#10 +
              'Түүнийг зогсоож, Zev Detector доторх хувилбараар СОЛИХ уу?' + #13#10 +
              '(Тохиргоо хуулагдана, зургууд хэвээр үлдэнэ. Үгүй бол "Суулгахгүй"-г сонгоно уу.)',
              mbConfirmation, MB_YESNO or MB_DEFBUTTON2) <> IDYES then
    begin
      Result := False;
      exit;
    end;
  end;
  if (CurPageID = ParkPage.ID) and ParkingWanted() and not KeepBox.Checked then
  begin
    if (Trim(TokenEdit.Text) = '') or (Trim(BarilgaEdit.Text) = '') then
    begin
      MsgBox('Token болон барилгын ID-г оруулна уу.', mbError, MB_OK);
      Result := False;
    end
    else if CheckedCameraCount() = 0 then
      Result := MsgBox('Камер сонгоогүй байна. Камергүйгээр үргэлжлүүлэх үү? (дараа нь config.toml-д нэмж болно)',
                       mbConfirmation, MB_YESNO) = IDYES;
  end;
end;

{ Arguments for `zev-detector.exe setup` (run from [Run] before the service starts).
  No camera ticked = camera list left as it was (an upgrade keeps its cameras). }
function GetSetupParams(Param: String): String;
var
  I: Integer;
begin
  Result := 'setup --name "' + CleanArg(NamePage.Values[0]) + '"';
  for I := 0 to GetArrayLength(CamSpecs) - 1 do
    if CamList.Checked[I] then
      Result := Result + ' --camera "' + CamSpecs[I] + '"';
end;

{ Everything after the files are copied, in a fixed order:
  1. PC name + cameras into Zev Detector's config
  2. parking service config, registration, start (+ detector watches it)
  3. Zev Detector service install/start — last, so it starts with the final config }
procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep <> ssPostInstall then exit;
  WizardForm.StatusLabel.Caption := 'Тохиргоо хадгалж байна...';
  Exec(ExpandConstant('{app}\{#AgentExe}'), GetSetupParams(''), '', SW_HIDE, ewWaitUntilTerminated, Code);
  Log('zev-detector setup exit ' + IntToStr(Code));
  RunParkingSetup();
  WizardForm.StatusLabel.Caption := 'Zev Detector үйлчилгээг эхлүүлж байна...';
  Exec(ExpandConstant('{app}\{#AgentExe}'), 'install', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Log('zev-detector install exit ' + IntToStr(Code));
end;

{ An upgrade over a running service can't replace the exe while it is loaded,
  so stop it before copying files; CurStepChanged brings it back. The bundled
  parking service is stopped too when it runs from the install folder and is
  being replaced. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode, I: Integer;
  AgentPath, D: String;
begin
  Result := '';
  for I := 1 to 2 do
  begin
    D := ServiceExeDir(ParkService(I));
    if (D <> '') and (Pos(LowerCase(AddBackslash(WizardDirValue)), LowerCase(AddBackslash(D))) = 1) then
      if ((I = 1) and ParkIs('dahua')) or ((I = 2) and ParkIs('hikvision')) then
      begin
        Exec(ExpandConstant('{sys}\sc.exe'), 'stop ' + ParkService(I), '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
        Sleep(2500);
      end;
  end;
  AgentPath := ExpandConstant('{app}\{#AgentExe}');
  if FileExists(AgentPath) then
  begin
    Exec(AgentPath, 'stop', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    { Give the SCM a moment to actually release the binary. }
    Sleep(1500);
  end;
end;
