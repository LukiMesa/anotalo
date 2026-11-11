; Instalador de Transcriptor (Inno Setup 6). Lo compila scripts/build.ps1:
;   ISCC.exe /DAppVersion=0.3.0 packaging\installer.iss
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6B7C2D51-3A8E-4F5B-9C1D-2E4F6A8B0C13}
AppName=Transcriptor
AppVersion={#AppVersion}
AppVerName=Transcriptor {#AppVersion}
AppPublisher=LukiMesa
DefaultDirName={localappdata}\Programs\Transcriptor
DefaultGroupName=Transcriptor
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=Transcriptor-Setup-{#AppVersion}
SetupIconFile=..\calltx\ui\assets\icon.ico
UninstallDisplayIcon={app}\Transcriptor.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=force
RestartApplications=no

[Languages]
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "autostart"; Description: "Iniciar con Windows (minimizado junto al reloj)"; GroupDescription: "Opciones:"
Name: "desktopicon"; Description: "Crear un acceso directo en el escritorio"; GroupDescription: "Opciones:"; Flags: unchecked

[Files]
Source: "..\dist\Transcriptor\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Transcriptor"; Filename: "{app}\Transcriptor.exe"
Name: "{autodesktop}\Transcriptor"; Filename: "{app}\Transcriptor.exe"; Tasks: desktopicon
; Mismo nombre que usa la app para su ajuste "Iniciar con Windows" (sustituye al de la versión de desarrollo)
Name: "{userstartup}\Transcriptor de llamadas"; Filename: "{app}\Transcriptor.exe"; Parameters: "--minimized"; WorkingDir: "{app}"; Tasks: autostart

[Run]
Filename: "{app}\Transcriptor.exe"; Description: "Abrir Transcriptor"; Flags: nowait postinstall skipifsilent
; Actualización silenciosa: volver a abrirla minimizada
Filename: "{app}\Transcriptor.exe"; Parameters: "--minimized"; Flags: nowait; Check: WizardSilent

[InstallDelete]
; Sin la casilla de inicio: quitar un acceso directo antiguo (p. ej. de la versión de desarrollo)
Type: files; Name: "{userstartup}\Transcriptor de llamadas.lnk"; Check: not WizardIsTaskSelected('autostart')

[UninstallDelete]
Type: files; Name: "{userstartup}\Transcriptor de llamadas.lnk"

[Code]
function WebView2Installed(): Boolean;
var v: String;
begin
  Result :=
    RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) or
    RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) or
    RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v);
  Result := Result and (v <> '') and (v <> '0.0.0.0');
end;

var
  AppWasRunning: Boolean;  // se cerró la app para instalar
  InstallDone: Boolean;    // los archivos nuevos quedaron instalados

procedure CloseRunningApp();
var code: Integer;
begin
  // La app solo se oculta al cerrar su ventana: se termina para poder reemplazar sus archivos.
  // Sin /T: cuando la app lanza este instalador para actualizarse, el instalador es su hijo
  // y /T lo mataría también. El proceso de transcripción se llama igual y cae con /IM.
  if Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM Transcriptor.exe', '', SW_HIDE, ewWaitUntilTerminated, code) and (code = 0) then
    AppWasRunning := True;
  Sleep(800);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    InstallDone := True;
end;

procedure DeinitializeSetup();
var code: Integer; exe: String;
begin
  // Actualización silenciosa que falló o se canceló después de cerrar la app: volver a abrirla
  // (si terminó bien, la abre la sección [Run]).
  if WizardSilent and AppWasRunning and not InstallDone then
  begin
    try
      exe := ExpandConstant('{app}\Transcriptor.exe');
    except
      exe := ExpandConstant('{localappdata}\Programs\Transcriptor\Transcriptor.exe');
    end;
    if FileExists(exe) then
      Exec(exe, '--minimized', '', SW_SHOWNORMAL, ewNoWait, code);
  end;
end;

function InitializeUninstall(): Boolean;
begin
  CloseRunningApp();
  Result := True;
end;

function InitializeSetup(): Boolean;
var code: Integer;
begin
  CloseRunningApp();
  Result := True;
  if not WebView2Installed() then
  begin
    if MsgBox('Falta Microsoft Edge WebView2 Runtime, necesario para la ventana de la app.' + #13#10 +
              '¿Abrir la página de descarga de Microsoft?', mbConfirmation, MB_YESNO) = IDYES then
      ShellExec('open', 'https://go.microsoft.com/fwlink/p/?LinkId=2124703', '', '', SW_SHOWNORMAL, ewNoWait, code);
  end;
end;
