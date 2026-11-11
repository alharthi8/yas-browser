#ifndef AppVersion
  #define AppVersion "0.4.9"
#endif
#ifndef AppRoot
  #define AppRoot AddBackslash(SourcePath) + ".."
#endif
#ifndef AppExe
  #define AppExe AppRoot + "\Yas.exe"
#endif
#ifndef OutputDir
  #define OutputDir AppRoot + "\.."
#endif
#ifndef BootstrapperSha256
  #error Build with package.ps1 so the Microsoft Bootstrapper signature and hash are verified.
#endif
#ifndef AiRuntimeFiles
  #error Build with package.ps1 so the CPU-only AI runtime inventory, hashes, and license notices are verified.
#endif
#ifndef AiRuntimeManifestSha256
  #error A reviewed AI runtime manifest hash is required.
#endif
#if AiRuntimeManifestSha256 != "FE26D105E6D8A9B82DA1AACC652F3EC9509591560FF5EC3807150DD8375FDDC7"
  #error AI runtime manifest does not match this application release.
#endif

[Setup]
AppId={{E290A0CE-DB17-4BD2-9724-90AB04BC61F7}
AppName=Yas Browser
AppVersion={#AppVersion}
AppVerName=Yas Browser {#AppVersion}
AppPublisher=Yas Browser
AppCopyright=Yas Browser
DefaultDirName={localappdata}\Programs\Yas
DefaultGroupName=Yas Browser
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
UninstallDisplayName=Yas Browser
UninstallDisplayIcon={app}\Yas.exe
UninstallFilesDir={app}\uninstall
OutputDir={#OutputDir}
OutputBaseFilename=Yas-{#AppVersion}-Setup-x64
SetupIconFile={#AppRoot}\branding\yas.ico
WizardStyle=modern
WizardImageFile=assets\wizard.bmp
WizardSmallImageFile=assets\wizard-small.bmp
WizardImageStretch=yes
Compression=lzma2/ultra64
SolidCompression=yes
DisableWelcomePage=no
DisableProgramGroupPage=yes
CloseApplications=no
RestartApplications=no
CloseApplicationsFilter=Yas.exe,GoodBro.exe
UsePreviousAppDir=no
UsePreviousGroup=no
UsePreviousTasks=yes
VersionInfoVersion={#AppVersion}.0
VersionInfoDescription=Yas browser installer
VersionInfoProductName=Yas Browser
VersionInfoProductVersion={#AppVersion}
SetupLogging=yes
ChangesAssociations=no
ChangesEnvironment=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[LangOptions]
DialogFontName=Dubai
WelcomeFontName=Dubai

[Messages]
WelcomeLabel1=Meet Yas Browser.
WelcomeLabel2=A clearer view of the web.%n%nSetup will install Yas Browser for your Windows account. Your bookmarks, browsing profile, and encrypted vault are stored separately from the application.%n%nMicrosoft Edge WebView2 Runtime is required. Your default browser will not change.
FinishedLabel=Yas is ready.%n%nYour good side of the web is one click away.

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
; Keep this first so ExtractTemporaryFile does not decompress the app before prerequisite setup.
Source: "{#AppRoot}\installer\prerequisites\MicrosoftEdgeWebview2Setup.exe"; DestDir: "{app}\prerequisites"; Flags: ignoreversion
Source: "{#AppRoot}\installer\prerequisites\README.txt"; DestDir: "{app}\prerequisites"; Flags: ignoreversion
Source: "{#AppRoot}\installer\prerequisites\WEBVIEW2_BOOTSTRAPPER.json"; DestDir: "{app}\prerequisites"; Flags: ignoreversion
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "Yas.exe"; Flags: ignoreversion; BeforeInstall: VerifyApplicationTargetBeforeCopy
Source: "{#AppRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppRoot}\RELEASE_NOTES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppRoot}\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppRoot}\RENAME_NOTES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppRoot}\UPDATES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppRoot}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#AppRoot}\assistant\*"; DestDir: "{app}\assistant"; Flags: ignoreversion recursesubdirs createallsubdirs
; package.ps1 copies only the reviewed manifest inventory to this isolated stage.
; This payload contains runtime binaries and notices, never downloaded models.
Source: "{#AiRuntimeFiles}\*"; DestDir: "{app}\ai-runtime"; Flags: ignoreversion recursesubdirs createallsubdirs; BeforeInstall: VerifyApplicationTargetBeforeCopy
#ifdef RuntimeFiles
Source: "{#RuntimeFiles}\vcruntime140.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RuntimeFiles}\vcruntime140_1.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RuntimeFiles}\README.txt"; DestDir: "{app}"; DestName: "MSVC-RUNTIME.txt"; Flags: ignoreversion skipifsourcedoesntexist
#endif

[Icons]
Name: "{autoprograms}\Yas Browser"; Filename: "{app}\Yas.exe"; WorkingDir: "{app}"; AppUserModelID: "Yas.Browser"
Name: "{autodesktop}\Yas Browser"; Filename: "{app}\Yas.exe"; WorkingDir: "{app}"; Tasks: desktopicon; AppUserModelID: "Yas.Browser"

[Run]
Filename: "{app}\Yas.exe"; Description: "Open Yas Browser"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

; Application files and shortcuts are removed by Inno Setup's uninstall log.
; Deliberately no UninstallDelete entries for user profile/history/vault paths.

[Code]
const
  WebViewClientKey = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  WebViewDownloadUrl = 'https://developer.microsoft.com/microsoft-edge/webview2/#download-section';
  ProductUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{E290A0CE-DB17-4BD2-9724-90AB04BC61F7}_is1';
var
  LegacyInstallDir, LegacyUninstaller, LegacyUpgradeError: String;
  LegacyWasRemoved: Boolean;

function HasRuntimeVersion(RootKey: Integer): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(RootKey, WebViewClientKey, 'pv', Version) and
    (Version <> '') and (Version <> '0.0.0.0');
end;

function HasWebView2: Boolean;
begin
  Result := HasRuntimeVersion(HKCU32) or HasRuntimeVersion(HKLM32);
  if IsWin64 then
    Result := Result or HasRuntimeVersion(HKCU64) or HasRuntimeVersion(HKLM64);
end;

function PrepareRuntime(var NeedsRestart: Boolean): String;
var
  ResultCode, Attempt: Integer;
  Bootstrapper: String;
begin
  Result := '';
  if HasWebView2 then begin
    Log('WebView2 Runtime detected; bundled Bootstrapper was not executed.');
    Exit;
  end;
  if WizardSilent then begin
    Result := 'Microsoft Edge WebView2 Runtime is missing. Run Yas Setup interactively to choose runtime installation, or install the Evergreen Runtime from ' + WebViewDownloadUrl + ' and retry.';
    Log(Result);
    Exit;
  end;
  if MsgBox('Yas needs Microsoft Edge WebView2 Runtime.' + #13#10#13#10 +
    'Install it now using the bundled Microsoft-signed setup helper?' + #13#10#13#10 +
    'An internet connection is required. Microsoft will download the Runtime for this computer. Choose No to stop without installing Yas.',
    mbConfirmation, MB_YESNO) <> IDYES then begin
    Result := 'WebView2 Runtime installation was declined. Yas has not been installed. Install the Evergreen Runtime from ' + WebViewDownloadUrl + ' and retry.';
    Exit;
  end;
  try
    ExtractTemporaryFile('MicrosoftEdgeWebview2Setup.exe');
    Bootstrapper := ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe');
    if CompareText(GetSHA256OfFile(Bootstrapper), '{#BootstrapperSha256}') <> 0 then begin
      Result := 'The bundled Microsoft Runtime helper failed its integrity check. Download a fresh Yas installer.';
      Exit;
    end;
    WizardForm.StatusLabel.Caption := 'Installing Microsoft Edge WebView2 Runtime. Internet access is required...';
    Log('User consented to install WebView2 using bundled Microsoft-signed Bootstrapper.');
    if not Exec(Bootstrapper, '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then begin
      Result := 'Microsoft WebView2 setup could not start. Windows error ' + IntToStr(ResultCode) + '. Install the Runtime from ' + WebViewDownloadUrl + ' and retry.';
      Exit;
    end;
    Log('Microsoft WebView2 setup exited with code ' + IntToStr(ResultCode) + '.');
    if ResultCode = 3010 then begin
      NeedsRestart := True;
      Result := 'Microsoft WebView2 setup requires a Windows restart. Restart Windows, then run Yas Setup again. Yas has not been installed yet.';
      Exit;
    end;
    for Attempt := 1 to 10 do begin
      if HasWebView2 then Break;
      Sleep(500);
    end;
    if not HasWebView2 then
      Result := 'Microsoft WebView2 Runtime was not detected after setup (exit ' + IntToStr(ResultCode) + '). Check your connection or install from ' + WebViewDownloadUrl + ' and retry.';
  except
    Result := 'Could not prepare Microsoft WebView2 Runtime: ' + GetExceptionMessage + '. Install it from ' + WebViewDownloadUrl + ' and retry.';
  end;
end;


// Keep the stable AppId for upgrades. Only retire the registered legacy product;
// its uninstaller removes logged application files, never the separate profiles.

function InitializeSetup: Boolean;
var
  DisplayName, RegisteredUninstaller, UninstallerName: String;
begin
  Result := True;
  LegacyWasRemoved := False;
  if not RegQueryStringValue(HKCU64, ProductUninstallKey, 'DisplayName', DisplayName) then Exit;
  if CompareText(DisplayName, 'GoodBro') <> 0 then Exit;
  if not RegQueryStringValue(HKCU64, ProductUninstallKey, 'InstallLocation', LegacyInstallDir) or
     not RegQueryStringValue(HKCU64, ProductUninstallKey, 'UninstallString', RegisteredUninstaller) then begin
    LegacyUpgradeError := 'The previous browser registration is incomplete. Remove its application using Windows Installed Apps, then run Yas Setup again. Your browser data is separate and must be kept.';
    Exit;
  end;
  LegacyUninstaller := RemoveQuotes(RegisteredUninstaller);
  UninstallerName := ExtractFileName(LegacyUninstaller);
  if (LegacyInstallDir = '') or
     (CompareText(ExtractFilePath(LegacyUninstaller), AddBackslash(LegacyInstallDir) + 'uninstall\') <> 0) or
     (CompareText(Copy(UninstallerName, 1, 5), 'unins') <> 0) or
     (CompareText(ExtractFileExt(UninstallerName), '.exe') <> 0) or
     not FileExists(LegacyUninstaller) then begin
    LegacyUpgradeError := 'The previous browser uninstaller could not be verified inside its registered installation folder. Remove its application using Windows Installed Apps, then retry Yas Setup. Your browser data is separate and must be kept.';
    LegacyUninstaller := '';
    Exit;
  end;
end;

function RetireLegacyInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := LegacyUpgradeError;
  if (Result <> '') or LegacyWasRemoved or (LegacyUninstaller = '') then Exit;
  if not FileExists(LegacyUninstaller) and not FileExists(AddBackslash(LegacyInstallDir) + 'GoodBro.exe') then begin
    LegacyWasRemoved := True;
    Exit;
  end;
  Log('Retiring the registered legacy browser application; separate browser data is preserved.');
  WizardForm.StatusLabel.Caption := 'Updating the previous browser application to Yas...';
  if not Exec(LegacyUninstaller, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-', LegacyInstallDir,
    SW_HIDE, ewWaitUntilTerminated, ResultCode) then begin
    Result := 'The previous browser uninstaller could not start. Close the previous browser and retry Yas Setup. Windows error ' + IntToStr(ResultCode) + '.';
    Exit;
  end;
  if (ResultCode <> 0) and (ResultCode <> 3010) then begin
    Result := 'The previous browser could not be updated (uninstaller exit ' + IntToStr(ResultCode) + '). Close it and retry Yas Setup. Browser data has not been removed.';
    Exit;
  end;
  if ResultCode = 3010 then NeedsRestart := True;
  if FileExists(AddBackslash(LegacyInstallDir) + 'GoodBro.exe') then begin
    Result := 'The previous browser is still in use. Close it, restart Windows if requested, then run Yas Setup again. Browser data has been preserved.';
    Exit;
  end;
  LegacyWasRemoved := True;
  Log('Registered legacy browser application removed; Yas will use the new installation folder.');
end;

// A running mapped executable cannot be opened for an exclusive update. This
// probe writes no bytes and also conservatively rejects other locks/permissions.
// Rechecking before the app copy reduces the check/copy window; it is not an
// atomic lock across the whole install. Setup never closes a download manager.
function ApplicationImageAvailable(const FileName: String): Boolean;
var
  Probe: TFileStream;
begin
  Result := True;
  if not FileExists(FileName) then Exit;
  Probe := nil;
  try
    try
      Probe := TFileStream.Create(FileName, fmOpenReadWrite or fmShareExclusive);
    finally
      if Probe <> nil then Probe.Free;
    end;
  except
    Result := False;
  end;
end;

function FolderImagesAvailable(const Directory: String): Boolean;
var
  FindRec: TFindRec;
  Mask: String;
  Kind: Integer;
begin
  Result := True;
  for Kind := 0 to 1 do begin
    if Kind = 0 then Mask := '*.exe' else Mask := '*.dll';
    if FindFirst(AddBackslash(Directory) + Mask, FindRec) then begin
      try
        repeat
          if not ApplicationImageAvailable(AddBackslash(Directory) + FindRec.Name) then begin
            Result := False;
            Exit;
          end;
        until not FindNext(FindRec);
      finally
        FindClose(FindRec);
      end;
    end;
  end;
end;

function AiRuntimeTargetsAvailable: Boolean;
begin
  Result := FolderImagesAvailable(ExpandConstant('{app}\ai-runtime')) and
    FolderImagesAvailable(ExpandConstant('{app}\ai-runtime\lib\ollama'));
end;

function InitializeUninstall: Boolean;
begin
  Result := ApplicationImageAvailable(ExpandConstant('{app}\Yas.exe')) and AiRuntimeTargetsAvailable;
  if not Result then begin
    Log('Uninstall deferred because the Yas application image is still in use or unwritable.');
    SuppressibleMsgBox('Yas or its local AI runtime is still running, including in the background, or an application file is in use. Let downloads and AI tasks finish, then exit Yas explicitly before uninstalling. Uninstall has not stopped any transfers. If Yas is already closed, check the application folder permissions and retry.', mbError, MB_OK, IDOK);
  end;
end;

function VerifyNoDowngrade: String;
var
  SetupVersion, ExistingVersion: Int64;
  RegisteredName, RegisteredVersion: String;
begin
  Result := '';
  if not StrToVersion('{#AppVersion}.0', SetupVersion) then begin
    Result := 'The installer version is invalid. Download a current Yas installer and retry.';
    Exit;
  end;
  if GetPackedVersion(ExpandConstant('{app}\Yas.exe'), ExistingVersion) and
     (ComparePackedVersion(ExistingVersion, SetupVersion) > 0) then begin
    Result := 'A newer version of Yas is already installed. Use the same or a newer installer. Setup has not changed the application or browser data.';
    Exit;
  end;
  if RegQueryStringValue(HKCU64, ProductUninstallKey, 'DisplayName', RegisteredName) and
     ((CompareText(RegisteredName, 'Yas') = 0) or
      (CompareText(RegisteredName, 'Yas Browser') = 0)) and
     RegQueryStringValue(HKCU64, ProductUninstallKey, 'DisplayVersion', RegisteredVersion) and
     StrToVersion(RegisteredVersion, ExistingVersion) and
     (ComparePackedVersion(ExistingVersion, SetupVersion) > 0) then
    Result := 'A newer version of Yas is registered for this account. Use the same or a newer installer. Setup has not changed the application or browser data.';
end;

function VerifyApplicationTargetsAvailable: String;
begin
  Result := LegacyUpgradeError;
  if Result <> '' then Exit;
  Result := VerifyNoDowngrade;
  if Result <> '' then Exit;
  if not ApplicationImageAvailable(ExpandConstant('{app}\Yas.exe')) then begin
    Result := 'Yas is still running, including in the background, or its application file is in use. Let downloads finish, then exit Yas explicitly (use tray Exit and stop downloads when available) before updating. Setup has not stopped any transfers. If Yas is already closed, check the application folder permissions and retry.';
    Exit;
  end;
  if not AiRuntimeTargetsAvailable then begin
    Result := 'A Yas local AI runtime file is still in use or unwritable. Let AI tasks finish, then exit Yas explicitly before updating. Setup has not stopped any transfers. If Yas is already closed, check the application folder permissions and retry.';
    Exit;
  end;
  if (LegacyUninstaller <> '') and not LegacyWasRemoved and
     not ApplicationImageAvailable(AddBackslash(LegacyInstallDir) + 'GoodBro.exe') then
    Result := 'The previous browser application is still running or its file is in use. Close it explicitly after letting downloads finish, then retry. Setup has not stopped any transfers.';
end;

procedure VerifyApplicationTargetBeforeCopy;
var
  Error: String;
begin
  Error := VerifyApplicationTargetsAvailable;
  if Error <> '' then RaiseException(Error);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := VerifyApplicationTargetsAvailable;
  if Result = '' then Result := PrepareRuntime(NeedsRestart);
  if Result = '' then Result := VerifyApplicationTargetsAvailable;
  if Result = '' then Result := RetireLegacyInstall(NeedsRestart);
end;
