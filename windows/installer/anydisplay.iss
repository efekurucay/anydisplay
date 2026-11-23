; AnyDisplay Windows installer (Inno Setup 6)
; Build locally from windows\:
;   cargo build --release
;   iscc installer\anydisplay.iss

#ifndef AppVersion
  #define AppVersion "0.1.4"
#endif

#ifndef OutputBaseFilename
  #define OutputBaseFilename "AnyDisplay-windows-x64-setup"
#endif

[Setup]
AppId={{8E9D5C4A-3A7B-4F81-9E42-6D1C2B5A7E90}
AppName=AnyDisplay
AppVersion={#AppVersion}
AppVerName=AnyDisplay {#AppVersion}
AppPublisher=AnyDisplay
AppPublisherURL=https://efekurucay.com/anydisplay/
AppSupportURL=https://github.com/efekurucay/anydisplay/issues
AppUpdatesURL=https://github.com/efekurucay/anydisplay/releases
DefaultDirName={autopf}\AnyDisplay
DefaultGroupName=AnyDisplay
DisableProgramGroupPage=yes
AllowNoIcons=yes
LicenseFile=..\..\LICENSE
OutputDir=out
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile=..\assets\app.ico
WizardSmallImageFile=..\assets\wizard-small.bmp
UninstallDisplayIcon={app}\anydisplay-gui.exe
UninstallDisplayName=AnyDisplay
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel2=This will install AnyDisplay on your PC so you can use any Android phone or tablet as a second monitor, touch screen, and pen tablet.%n%nFor extended display, follow the Virtual Display Driver setup in the AnyDisplay README. Setup can install the optional USB tools.
WizardSelectTasks=Customize Your Setup
SelectTasksDesc=Choose the extras you want to include.
SelectTasksLabel2=Recommended options are already checked. Click Next when you're ready.
FinishedHeadingLabel=AnyDisplay is ready!
FinishedLabel=AnyDisplay has been installed on your computer.%n%nOpen AnyDisplay on your Android device to connect over Wi-Fi or USB.%n%nIf you added display resolutions, reload the driver or restart Windows to apply them.

[Tasks]
Name: "vddresolutions"; Description: "Add Android tablet && phone resolutions (driver reload required)"; GroupDescription: "Display & USB setup:"; Check: ShouldOfferVddResolutions
Name: "installadb"; Description: "Enable USB cable connections (installs Android Platform-Tools / adb via winget)"; GroupDescription: "Display & USB setup:"; Check: ShouldOfferAdbInstall
Name: "autostart"; Description: "Launch AnyDisplay automatically when Windows starts"; GroupDescription: "Shortcuts & startup:"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts & startup:"; Flags: unchecked
Name: "addtopath"; Description: "Use AnyDisplay from the command line (adds anydisplay-win to PATH)"; GroupDescription: "Developer tools:"

[Files]
Source: "..\target\release\anydisplay-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\anydisplay-win.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "add-vdd-resolutions.ps1"; DestDir: "{tmp}"; Flags: deleteafterinstall; Tasks: vddresolutions

[Icons]
Name: "{group}\AnyDisplay"; Filename: "{app}\anydisplay-gui.exe"; Comment: "Turn an Android phone or tablet into a second monitor"
Name: "{group}\Uninstall AnyDisplay"; Filename: "{uninstallexe}"
Name: "{autodesktop}\AnyDisplay"; Filename: "{app}\anydisplay-gui.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "AnyDisplay"; ValueData: """{app}\anydisplay-gui.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{localappdata}\Microsoft\WindowsApps\winget.exe"; Parameters: "install --id=Google.PlatformTools -e --accept-source-agreements --accept-package-agreements"; StatusMsg: "Installing Android Platform-Tools (adb) via winget..."; Flags: waituntilterminated; Tasks: installadb
Filename: "{app}\anydisplay-gui.exe"; Description: "{cm:LaunchProgram,AnyDisplay}"; Flags: nowait postinstall skipifsilent

[Code]
const
  InstallerKey = 'Software\AnyDisplay\Installer';
  PathValueName = 'AddedPath';

function HasWinget: Boolean;
begin
  Result := FileExists(ExpandConstant('{localappdata}\Microsoft\WindowsApps\winget.exe'));
end;

function IsAdbInstalled: Boolean;
var
  EnvPath, Entry: String;
  P: Integer;
begin
  Result :=
    FileExists(ExpandConstant('{localappdata}\Android\Sdk\platform-tools\adb.exe')) or
    FileExists(ExpandConstant('{localappdata}\Microsoft\WinGet\Links\adb.exe')) or
    FileExists('C:\ProgramData\chocolatey\bin\adb.exe') or
    FileExists(ExpandConstant('{%USERPROFILE}\scoop\shims\adb.exe')) or
    FileExists('C:\platform-tools\adb.exe');
  if Result then
    Exit;

  EnvPath := GetEnv('PATH');
  while Length(EnvPath) > 0 do
  begin
    P := Pos(';', EnvPath);
    if P > 0 then
    begin
      Entry := Copy(EnvPath, 1, P - 1);
      Delete(EnvPath, 1, P);
    end
    else
    begin
      Entry := EnvPath;
      EnvPath := '';
    end;
    if (Entry <> '') and FileExists(AddBackslash(Entry) + 'adb.exe') then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

function ShouldOfferVddResolutions: Boolean;
begin
  Result := FileExists('C:\VirtualDisplayDriver\vdd_settings.xml');
end;

function ShouldOfferAdbInstall: Boolean;
begin
  Result := (not IsAdbInstalled) and HasWinget;
end;

procedure GetEnvRootAndKey(var RootKey: Integer; var SubKey: String);
begin
  if IsAdminInstallMode then
  begin
    RootKey := HKEY_LOCAL_MACHINE;
    SubKey := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end
  else
  begin
    RootKey := HKEY_CURRENT_USER;
    SubKey := 'Environment';
  end;
end;

procedure AddToPath(Dir: String);
var
  RootKey: Integer;
  SubKey, PathVal, LowerPath, LowerDir: String;
begin
  GetEnvRootAndKey(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', PathVal) then
    PathVal := '';

  LowerPath := ';' + Lowercase(PathVal) + ';';
  LowerDir := ';' + Lowercase(Dir) + ';';
  if Pos(LowerDir, LowerPath) > 0 then
    Exit;

  if PathVal <> '' then
    PathVal := PathVal + ';';
  PathVal := PathVal + Dir;
  if RegWriteStringValue(RootKey, InstallerKey, PathValueName, Dir) then
    if not RegWriteExpandStringValue(RootKey, SubKey, 'Path', PathVal) then
      RegDeleteValue(RootKey, InstallerKey, PathValueName);
end;

procedure RemoveFromPath;
var
  RootKey: Integer;
  SubKey, PathVal, Dir: String;
  P: Integer;
begin
  GetEnvRootAndKey(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, InstallerKey, PathValueName, Dir) then
    Exit;

  if RegQueryStringValue(RootKey, SubKey, 'Path', PathVal) then
  begin
    PathVal := ';' + PathVal + ';';
    P := Pos(';' + Lowercase(Dir) + ';', Lowercase(PathVal));
    if P > 0 then
    begin
      Delete(PathVal, P, Length(Dir) + 1);
      Delete(PathVal, Length(PathVal), 1);
      Delete(PathVal, 1, 1);
      if not RegWriteExpandStringValue(RootKey, SubKey, 'Path', PathVal) then
        Exit;
    end;
  end;
  RegDeleteValue(RootKey, InstallerKey, PathValueName);
  RegDeleteKeyIfEmpty(RootKey, InstallerKey);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep <> ssPostInstall then
    Exit;

  if WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));

  if WizardIsTaskSelected('vddresolutions') then
  begin
    if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -ExecutionPolicy Bypass -File "' +
      ExpandConstant('{tmp}\add-vdd-resolutions.ps1') + '"', '', SW_HIDE,
      ewWaitUntilTerminated, ResultCode) or (ResultCode <> 0) then
      SuppressibleMsgBox('Setup could not add display resolutions. Configure them in Virtual Driver Control and reload the driver.',
        mbError, MB_OK, IDOK);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath;
end;
