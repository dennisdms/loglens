; ---------------------------------------------------------------------------
; Log Lens — Windows per-user installer (Inno Setup).
;
; Plan: docs/plans/d1-distribution-pipeline.md, step 4 (4.1, 4.3, 4.4, 4.5).
; Decisions: docs/adr/0003-user-scope-distribution.md.
;
; Per-user install, modelled on the VS Code User Installer. Nothing here
; writes outside the user's own profile, so no UAC prompt is raised and the
; install directory stays writable by the app itself — which is what makes the
; in-app Update possible without elevation.
;
; ---------------------------------------------------------------------------
; HOW THE RELEASE WORKFLOW INVOKES THIS  (for the `build-windows` job)
; ---------------------------------------------------------------------------
; Inno Setup 6 is preinstalled on GitHub's Windows runners (6.4.0 on
; windows-2022, 6.7.1 on windows-2025) and `iscc` is on PATH. If that ever
; stops being true: `choco install innosetup -y`. Inno Setup **6.3 or newer**
; is required — `ArchitecturesAllowed=x64compatible` and the Pascal function
; `SaveStringsToUTF8FileWithoutBOM` both arrived in 6.3.0.
;
; From the repository root, after `cargo build --release`:
;
;     iscc /DAppVersion=0.1.0 packaging\windows\loglens.iss
;
; `/DAppVersion=` is the one define the workflow must pass; it is the version
; without the leading `v` of the tag (step 6.1 has already proved the tag and
; Cargo.toml agree). `SourceExe` and `OutputDir` below have working defaults
; and only need overriding if the workflow stages the binary somewhere else:
;
;     iscc /DAppVersion=0.1.0 /DSourceExe=staging\loglens.exe ^
;          /DOutputDir=artifacts packaging\windows\loglens.iss
;
; Both are resolved relative to this script's directory (Inno's SourceDir),
; not the current directory. `iscc` exits 0 on success, 2 on a failed compile.
;
; The setup Artifact lands at, relative to the repository root:
;
;     dist\LogLens-<version>-windows-x86_64-setup.exe
;
; The portable Artifact is staged and zipped by the sibling script:
;
;     pwsh packaging\windows\make-portable.ps1 -Version 0.1.0
;     -> dist\LogLens-<version>-windows-x86_64-portable.zip
;
; Upload both from `dist\`. Their names are a compatibility contract (4.4);
; do not let the workflow rename them on the way out.
; ---------------------------------------------------------------------------

#ifndef AppVersion
  ; Only so the script still compiles when opened by hand. Every real build
  ; passes /DAppVersion= — a Release must never carry this placeholder.
  #define AppVersion "0.0.0-dev"
#endif

#ifndef SourceExe
  #define SourceExe "..\..\target\release\loglens.exe"
#endif

#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

#define AppName "Log Lens"
#define BinName "loglens.exe"

[Setup]
; The doubled brace is Inno's escape for a literal `{`, so the AppId is
; `{io.github.dennisdms.LogLens}` and the per-user uninstall registry key is
; `{io.github.dennisdms.LogLens}_is1`. It must never change: a different AppId
; means a second Add/Remove Programs entry instead of an upgrade of the first.
AppId={{io.github.dennisdms.LogLens}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
UninstallDisplayName={#AppName}
AppPublisher=dennisdms
AppPublisherURL=https://github.com/dennisdms/loglens
AppSupportURL=https://github.com/dennisdms/loglens/issues
AppUpdatesURL=https://github.com/dennisdms/loglens/releases

; The no-admin guarantee. `lowest` means Setup never asks to be elevated, even
; when an administrator runs it: it always runs in non administrative install
; mode, so {app} lands under the user's own profile, {group} lands in the
; user's Start Menu, and the uninstall entry lands in HKCU. Everything else in
; this file follows from that one line.
; PrivilegesRequired is deliberately left un-overridable — with
; PrivilegesRequiredOverridesAllowed unset (the default), /ALLUSERS on the
; command line cannot talk this installer into an elevated, machine-wide
; install that the app would then be unable to update.
PrivilegesRequired=lowest

DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
; Default is yes; stated explicitly because the in-app Update depends on it.
; A silent re-run must reinstall over the directory the previous install
; recorded, not into a fresh default — otherwise the Update would leave a
; second copy behind and the running one untouched.
UsePreviousAppDir=yes

; x64compatible, not x64: it matches Arm64 Windows 11, which runs x64 binaries
; under emulation. Requires Inno Setup 6.3+.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

SetupIconFile=..\..\assets\app-icon\icon.ico
UninstallDisplayIcon={app}\{#BinName}
WizardStyle=modern

; The Restart Manager. Windows will not let a running .exe be overwritten, and
; an in-app Update is by definition run while the app is running: it downloads
; this installer and spawns it with /SILENT /NORESTART. CloseApplications=yes
; lets Setup close Log Lens first; RestartApplications=yes brings it back
; afterwards. Neither is optional — without them a silent update fails on a
; locked file, and without the second the user's app simply vanishes.
CloseApplications=yes
RestartApplications=yes

OutputDir={#OutputDir}
; Artifact naming is a contract (plan 4.4) — the Update check matches Release
; assets by name, so renaming this breaks self-update for every already
; installed copy, the one population that cannot fix itself.
OutputBaseFilename=LogLens-{#AppVersion}-windows-x86_64-setup

; No code signing (ADR 0003). SmartScreen will warn on first run; that is
; documented, not worked around.

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Unchecked by default: a desktop icon nobody asked for is clutter. The Start
; menu entry below is not a task — it is always created.
; Note that UsePreviousTasks defaults to yes, so a silent Update keeps whatever
; the user chose the first time rather than quietly dropping their shortcut.
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

; No PATH task. Deliberately: not offered, not defaulted. Log Lens is a GUI
; app launched from the Start menu, and an installer that edits the user's
; environment to add a directory they did not ask for earns a bug report.

[Files]
; AfterInstall, not just CurStepChanged(ssPostInstall): Setup relaunches the
; applications the Restart Manager closed *before* it reaches ssPostInstall
; (Setup.MainForm.pas — RestartApplications, then SetStep(ssPostInstall)). On
; a silent Update that means Log Lens is already back up by then, so the
; marker has to be on disk by the time the file lands. WriteInstallManifest is
; idempotent and is called from both hooks; see [Code].
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#BinName}"; Flags: ignoreversion; AfterInstall: WriteInstallManifest

[Icons]
; {group} resolves to the current user's Start Menu because of
; PrivilegesRequired=lowest. This is the entry that makes Log Lens findable by
; typing its name into Start.
Name: "{group}\{#AppName}"; Filename: "{app}\{#BinName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#BinName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
; Offer to launch after an interactive install. `skipifsilent` keeps this out
; of the Update path, where the Restart Manager is what relaunches the app —
; running both would leave the user with two copies open.
Filename: "{app}\{#BinName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The Install flavour marker is written by [Code] after the install, not by
; [Files], so nothing in the uninstall log knows about it and it has to be
; named here. Without this line the file — and therefore {app} — survives the
; uninstall.
;
; A `Type: dirifempty; Name: "{app}"` entry is deliberately NOT added after it.
; Inno already retries every directory it failed to remove once the uninstall
; data files are gone (Setup.UninstallLog.pas, LoggedProcessDirsNotRemoved),
; which is after this entry has run, so {app} is removed on that pass.
Type: files; Name: "{app}\install-manifest.json"

[Code]

// Everything below uses // comments rather than Pascal's { } block comments.
// Pascal block comments do not nest and end at the first }, so a comment that
// mentions {app} — as these have to — would terminate halfway through and turn
// its own prose into code.
//
// ---------------------------------------------------------------------------
// CONTRACT — the Install flavour marker (plan 4.3)
// ---------------------------------------------------------------------------
// Written to {app}\install-manifest.json, next to the binary:
//
//     {
//       "flavour": "installer",
//       "install_dir": "C:\\Users\\you\\AppData\\Local\\Programs\\Log Lens",
//       "version": "0.1.0"
//     }
//
// Exactly three fields, the same shape packaging/linux/install.sh writes.
// `install_dir` is the directory holding the binary — on Windows that is {app}
// itself, so the same rule holds on both platforms: a copy is
// Installer-managed only while `install_dir` equals the parent of
// `std::env::current_exe()`. Anything else — no file, unparseable file, a
// different directory — is Portable.
//
// The directory check is the whole point. A portable copy run on a machine
// that also has Log Lens installed must not find a marker and believe it may
// update itself in place; on Windows the marker sits inside {app}, so a
// portable copy will not even see one.
//
// Two encoding details the app depends on:
//
// - Every backslash in the path is doubled. `C:\Users` inside a JSON string
//   literal is an invalid escape sequence and serde_json rejects the whole
//   file, which would silently demote a real installation to Portable.
// - The file is UTF-8 with no BOM. A BOM is not whitespace to a JSON parser;
//   serde_json fails on it, and a user name with non-ASCII characters is
//   perfectly ordinary. Hence SaveStringsToUTF8FileWithoutBOM (Inno 6.3+)
//   rather than SaveStringToFile, whose parameter is an AnsiString.
procedure WriteInstallManifest;
var
  InstallDir: String;
  ManifestPath: String;
  Lines: TArrayOfString;
begin
  InstallDir := ExpandConstant('{app}');

  // Backslash first, then quote: the other order would re-double the backslash
  // that escaping a quote had just introduced. A double quote cannot legally
  // appear in a Windows path, but the second line costs nothing and removes
  // the need to be certain of that.
  StringChangeEx(InstallDir, '\', '\\', True);
  StringChangeEx(InstallDir, '"', '\"', True);

  SetArrayLength(Lines, 5);
  Lines[0] := '{';
  Lines[1] := '  "flavour": "installer",';
  Lines[2] := '  "install_dir": "' + InstallDir + '",';
  Lines[3] := '  "version": "{#AppVersion}"';
  Lines[4] := '}';

  ManifestPath := ExpandConstant('{app}\install-manifest.json');
  if not SaveStringsToUTF8FileWithoutBOM(ManifestPath, Lines, False) then
    Log('Log Lens: failed to write ' + ManifestPath);
end;

// Belt and braces. The [Files] entry's AfterInstall is what gets the marker
// onto disk early enough for a Restart-Manager relaunch to see it; this second
// call is the guarantee that it exists at all even if that entry were ever
// skipped. Writing it twice is idempotent — the same three fields, truncated
// and rewritten.
procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    WriteInstallManifest;
end;

// ---------------------------------------------------------------------------
// What uninstall leaves behind (plan 4.5)
// ---------------------------------------------------------------------------
// Nothing here touches %APPDATA%\loglens\ — Rust's dirs::config_dir()/loglens/,
// holding config.json with every Connection and Saved Search — or any entry in
// Windows Credential Manager, where Connection secrets live. Reinstall-to-fix
// is the most common reason anyone uninstalls; wiping their configured
// Connections and stored credentials for that is hostile.
//
// So say where they were kept, in the same terms as
// packaging/linux/uninstall.sh does on the way out. Silent uninstalls say
// nothing: an in-app Update must never stop on a message box.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox('Your Connections and settings were kept in' + #13#10 +
           ExpandConstant('{userappdata}\loglens') + #13#10 +
           'and your stored Connection secrets were left in Windows' + #13#10 +
           'Credential Manager.' + #13#10 + #13#10 +
           'Delete that folder by hand if you want them gone too.',
           mbInformation, MB_OK);
end;
