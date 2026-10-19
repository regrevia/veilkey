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
OutputDir=.
OutputBaseFilename=VeilKey-Setup-{#AppVersion}-windows-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\veilkey.exe
ArchitecturesInstallIn64BitMode=x64
ArchitecturesAllowed=x64
LicenseFile={#SourceDir}\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#BinaryPath}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\VeilKey"; Filename: "{app}\veilkey.exe"
Name: "{autodesktop}\VeilKey"; Filename: "{app}\veilkey.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Run]
Filename: "{app}\veilkey.exe"; Description: "Launch VeilKey"; Flags: nowait postinstall skipifsilent
