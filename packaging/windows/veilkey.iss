#define AppName "VeilKey"
#define AppPublisher "Regrevia"
#define AppURL "https://github.com/regrevia/veilkey"
#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\\.."
#endif
#ifndef BinaryPath
  #define BinaryPath SourceDir + "\\target\\x86_64-pc-windows-msvc\\release\\veilkey.exe"
#endif

[Setup]
AppId={{A4C7E2E5-7B0F-4B9C-8D92-0B0E6DBA7D42}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
DefaultDirName={localappdata}\Programs\VeilKey
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
CloseApplications=no
RestartApplications=no
OutputDir=.
OutputBaseFilename=VeilKey-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\veilkey.exe
ArchitecturesInstallIn64BitMode=x64
ArchitecturesAllowed=x64
LicenseFile={#SourceDir}\LICENSE
SetupIconFile={#SourceDir}\assets\icons\veilkey.ico

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Files]
Source: "{#BinaryPath}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\VeilKey"; Filename: "{app}\veilkey.exe"; IconFilename: "{app}\veilkey.exe"; IconIndex: 0
Name: "{autodesktop}\VeilKey"; Filename: "{app}\veilkey.exe"; IconFilename: "{app}\veilkey.exe"; IconIndex: 0; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加选项："; Flags: unchecked

[Run]
Filename: "{app}\veilkey.exe"; Description: "启动 VeilKey"; Flags: nowait postinstall skipifsilent

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := '';
  // VeilKey stays in the notification area and deliberately ignores ordinary
  // WM_CLOSE, so stop the old instance before replacing its executable.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /T /IM veilkey.exe', '', SW_HIDE,
    ewWaitUntilTerminated, ResultCode);
  Sleep(750);
end;
