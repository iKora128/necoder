; necoder の Windows インストーラ（Inno Setup 6・WINDOWS-PORT.md §W7）。
;
; ## 何をするか
;
; - **ユーザー単位で入れる**（管理者権限・UAC 不要）。置き場は `%LOCALAPPDATA%\Programs\necoder`
;   （VS Code の User Installer と同じ流儀）。全ユーザー（Program Files）は `/ALLUSERS` を付けた時だけ
; - スタートメニューに necoder を登録する（検索して起動できる・ピン留めできる）
; - 選べる追加の作業（タスク）:
;     - デスクトップにショートカット（既定 ON）
;     - エクスプローラの右クリックに「necoder で開く」（フォルダ・フォルダの空き・ファイル。既定 ON）
;     - PATH に追加（ターミナルで `necoder .` と打てる。既定 ON）
; - 「アプリと機能」からアンインストールできる。**設定・履歴（%APPDATA% / %LOCALAPPDATA% の necoder）は消さない**
;   （入れ直しや上書き更新で会話の履歴が消えたら事故）
; - 上書きインストールで更新できる（起動中の necoder は閉じるよう求める）
;
; ## 使い方（ふつうは scripts/bundle-windows.ps1 と release.yml が呼ぶ）
;
;   ISCC.exe /DAppVersion=0.1.30 /DStageDir=<zip と同じ中身のフォルダ> /DOutputDir=<出力先> scripts\windows\necoder.iss
;
; 出力: <OutputDir>\necoder-windows-x64-setup.exe
;
; **未署名**なので、ブラウザで落とした setup.exe は初回に SmartScreen の青い画面が出る（README 参照）。
; このファイルは UTF-8（BOM 付き）で保つこと — Inno Setup は BOM で UTF-8 と判断する。

#ifndef AppVersion
  #error "/DAppVersion=x.y.z を渡すこと"
#endif
#ifndef StageDir
  #error "/DStageDir=<配布物を並べたフォルダ> を渡すこと"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#define AppName "necoder"
#define AppExe "necoder.exe"

[Setup]
; AppId は**変えない**（変えると別アプリ扱いになり、上書き更新・アンインストールが繋がらない）。
AppId={{05EA1B74-1149-410D-8BA0-5E6791B1DDA6}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=necoder
AppPublisherURL=https://necoder.com
AppSupportURL=https://github.com/iKora128/necoder/issues
AppUpdatesURL=https://github.com/iKora128/necoder/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} セットアップ
; lowest = 既定はユーザー単位（{autopf} が %LOCALAPPDATA%\Programs になる）。
; commandline = `/ALLUSERS` を付けた時だけ UAC を出して Program Files へ。dialog（起動直後に
; 「すべてのユーザー（推奨）／自分だけ」を聞く）は付けない — 「推奨」に釣られて UAC が出るのは摩擦なので。
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
WizardStyle=modern
SetupIconFile=..\..\crates\necoder\assets\icon\necoder.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=necoder-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
; 上書き更新で、起動中の necoder を閉じてから置き換える（使用中の exe は置き換えられない）。
CloseApplications=yes
RestartApplications=no
; PATH を書き換えたことを他のアプリへ知らせる（WM_SETTINGCHANGE）。
ChangesEnvironment=yes
ShowLanguageDialog=auto

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
japanese.TaskGroupIntegration=Windows との連携:
english.TaskGroupIntegration=Windows integration:
japanese.TaskContextMenu=エクスプローラの右クリックに「necoder で開く」を追加する
english.TaskContextMenu=Add "Open with necoder" to the Explorer context menu
japanese.TaskAddToPath=PATH に追加する（ターミナルで necoder . と打てるようになります）
english.TaskAddToPath=Add to PATH (lets you type necoder . in a terminal)
japanese.OpenWith=necoder で開く
english.OpenWith=Open with necoder

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "contextmenu"; Description: "{cm:TaskContextMenu}"; GroupDescription: "{cm:TaskGroupIntegration}"
Name: "addtopath"; Description: "{cm:TaskAddToPath}"; GroupDescription: "{cm:TaskGroupIntegration}"

[Files]
Source: "{#StageDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\*"; Excludes: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; 右クリックの「necoder で開く」。HKA = ユーザー単位なら HKCU・全ユーザーなら HKLM。
; Windows 11 では「その他のオプションを確認」の中に出る（新しいメニューへ出すには署名済みの
; パッケージ ID が要るので、署名を始めてから）。
Root: HKA; Subkey: "Software\Classes\Directory\shell\necoder"; ValueType: string; ValueName: ""; ValueData: "{cm:OpenWith}"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\necoder"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe}"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\necoder\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%V"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\necoder"; ValueType: string; ValueName: ""; ValueData: "{cm:OpenWith}"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\necoder"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe}"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\necoder\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%V"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\necoder"; ValueType: string; ValueName: ""; ValueData: "{cm:OpenWith}"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\*\shell\necoder"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe}"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\necoder\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
// PATH（ユーザー単位なら HKCU\Environment・全ユーザーなら HKLM の Session Manager\Environment）へ
// {app} を足す / 外す。既に在れば足さない（上書き更新で同じ行が増えないように）。
// 比較は Windows の流儀で大文字小文字と末尾の \ を区別しない。

function PathRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function PathKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function NormalizedDir(Dir: String): String;
begin
  Result := Lowercase(Trim(Dir));
  while (Length(Result) > 0) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

// Path を ; で区切った中に Dir が在るか。
function PathContains(Path, Dir: String): Boolean;
var
  Rest, Entry: String;
  Separator: Integer;
begin
  Result := False;
  Rest := Path;
  while Rest <> '' do
  begin
    Separator := Pos(';', Rest);
    if Separator = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end else
    begin
      Entry := Copy(Rest, 1, Separator - 1);
      Delete(Rest, 1, Separator);
    end;
    if NormalizedDir(Entry) = NormalizedDir(Dir) then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddToPath(Dir: String);
var
  Path: String;
begin
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Path) then
    Path := '';
  if PathContains(Path, Dir) then
    Exit;
  if (Path <> '') and (Path[Length(Path)] <> ';') then
    Path := Path + ';';
  // REG_EXPAND_SZ で書く（既存の %USERPROFILE% などを展開されないまま保つ）。
  RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Path, Rest, Entry, Kept: String;
  Separator: Integer;
begin
  if not RegQueryStringValue(PathRoot, PathKey, 'Path', Path) then
    Exit;
  Kept := '';
  Rest := Path;
  while Rest <> '' do
  begin
    Separator := Pos(';', Rest);
    if Separator = 0 then
    begin
      Entry := Rest;
      Rest := '';
    end else
    begin
      Entry := Copy(Rest, 1, Separator - 1);
      Delete(Rest, 1, Separator);
    end;
    if (Trim(Entry) <> '') and (NormalizedDir(Entry) <> NormalizedDir(Dir)) then
    begin
      if Kept <> '' then
        Kept := Kept + ';';
      Kept := Kept + Entry;
    end;
  end;
  if Kept <> Path then
    RegWriteExpandStringValue(PathRoot, PathKey, 'Path', Kept);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      AddToPath(ExpandConstant('{app}'))
    else
      // 前の版で足していて、今回は外した場合。
      RemoveFromPath(ExpandConstant('{app}'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
