; Lanwork 按用户安装。机制见 docs/architecture.md「安装程序与便携包」。
; 不请求管理员，不安装 WebView2，不安装 Windows App Runtime。
; 卸载不删除数据目录，也不删除 %LOCALAPPDATA%\Lanwork 下的引导文件和缓存。

#define MyAppVersion "0.5.2"

[Setup]
AppId={{539894DD-478A-43E4-874C-054665F0B0BA}
AppName=Lanwork
AppVersion={#MyAppVersion}
AppPublisher=Lanwork
VersionInfoVersion={#MyAppVersion}.0
VersionInfoProductVersion={#MyAppVersion}
VersionInfoProductName=Lanwork
VersionInfoDescription=Lanwork
VersionInfoCopyright=Copyright (c) 2026 Wynn
DefaultDirName={localappdata}\Programs\Lanwork
DefaultGroupName=Lanwork
DisableProgramGroupPage=yes
DisableWelcomePage=yes
AllowNoIcons=no
OutputDir=out
OutputBaseFilename=Lanwork-Setup-{#MyAppVersion}
SetupIconFile=..\crates\app\assets\lanwork.ico
UninstallDisplayIcon={app}\lanwork.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
UsedUserAreasWarning=no
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
; 6.1 是 Inno Setup 6.7.3 的编译器下限，不是产品最低版本。
MinVersion=6.1
AppMutex=Local\Lanwork.SingleInstance.Mutex
; 应用内更新在进程退出后使用 /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS。
; 不传 /DIR，不删除用户数据。静默安装发现互斥量还在就退出，所以要等进程结束再启动。
CloseApplications=yes
CloseApplicationsFilter=lanwork.exe
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "chinesesimplified"; MessagesFile: "ChineseSimplified.isl"

[Files]
Source: "staging\lanwork.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\Everything3_x64.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\Everything64.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\Everything-SDK3-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\Everything-SDK-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\Unicode-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "helper\lanwork-shortcut.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall

[Icons]
Name: "{userprograms}\Lanwork"; Filename: "{app}\lanwork.exe"; WorkingDir: "{app}"; IconFilename: "{app}\lanwork.exe"; Comment: "Lanwork"

[Registry]
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Lanwork"; ValueType: string; ValueName: "DisplayName"; ValueData: "Lanwork"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\Lanwork"; ValueType: string; ValueName: "CustomActivator"; ValueData: "{{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}"
Root: HKCU; Subkey: "Software\Classes\CLSID\{{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}"; ValueType: string; ValueName: ""; ValueData: "Lanwork"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\CLSID\{{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}\LocalServer32"; ValueType: string; ValueName: ""; ValueData: """{app}\lanwork.exe"""
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Lanwork"; Flags: uninsdeletevalue dontcreatekey

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
  { 静默安装时进程还在就退出，不结束 lanwork.exe。交互安装由 AppMutex 提示关掉。 }
  if WizardSilent and CheckForMutexes('Local\Lanwork.SingleInstance.Mutex') then
  begin
    Log('Lanwork is running; silent setup exits without stopping it.');
    Result := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  NeedsRestart := False;
  if Pos('"', ExpandConstant('{app}')) > 0 then
    Result := '安装目录不能包含引号。'
  else
    Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Params: String;
begin
  if CurStep <> ssPostInstall then
    Exit;
  Params := 'write "' + ExpandConstant('{app}\lanwork.exe') + '" "' + ExpandConstant('{userprograms}\Lanwork.lnk') + '"';
  if not Exec(ExpandConstant('{tmp}\lanwork-shortcut.exe'), Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    RaiseException('无法运行快捷方式写入。');
  if ResultCode <> 0 then
    RaiseException('快捷方式写入失败，退出码 ' + IntToStr(ResultCode) + '。');
end;
