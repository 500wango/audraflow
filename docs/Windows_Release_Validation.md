# Windows Release Validation

Run this checklist on a real Windows x64 machine before handing AudraFlow to testers.

## Prerequisites

- Windows 10 or 11 x64.
- Node.js 20.19+ or 22.12+ and npm 10+.
- Rust stable with the MSVC toolchain.
- Visual Studio Build Tools with the C++ desktop workload.
- PowerShell 5.1+.
- Bundled Windows runtime tools in `release\windows-portable\AudraFlow\bin` (ffmpeg, ffprobe, whisper-cli + DLLs).
- The bundled default Whisper base model is prepared automatically by `npm run prepare:runtime-assets` into:
  - `src-tauri\default-models\ggml-base.bin` (required for Tauri packaging)
  - `release\default-models\ggml-base.bin` (mirror for portable/docs checks)

## Build

From the repository root:

```powershell
npm install
npm run lint
npm run build
cargo test --workspace
npm run desktop:build:windows
```

The Windows installer artifacts are expected under:

```text
target\release\bundle
```

## Automated Package Check

Verify the portable package, installer artifacts, bundled tools, and a real packaged-orchestrator transcription smoke test:

```powershell
npm run release:verify:windows -- -BuildInstallers
```

If installers are already built:

```powershell
npm run release:verify:windows
```

To validate an installed copy after running the installer, pass the install directory:

```powershell
npm run release:verify:windows -- -InstalledAppDir "C:\Users\<you>\AppData\Local\Programs\AudraFlow"
```

The script runs `scripts\smoke-e2e.ps1` against the packaged `audraflow-orchestrator.exe`, not the debug build.

## Manual Install Check

Install the NSIS or MSI package and verify:

- AudraFlow launches from Start Menu and from the installed executable.
- The telemetry consent dialog responds to both choices.
- The language switch changes between English and Chinese.
- The bundled `base` model appears in Settings and is selected on a fresh profile.
- Runtime Health reports the default Whisper model, Whisper CLI, FFmpeg, FFprobe, and bundled `yt-dlp` as ready.
- For repairable Runtime Health rows, the repair button restores the default model or installs the managed dependency without requiring the user to find files manually.
- Model import or model download succeeds for an additional model.
- A short English audio file transcribes successfully.
- A short Chinese audio file transcribes successfully.
- Search finds transcript text.
- Glossary add, edit, and delete work.
- Export to TXT, Markdown, SRT, VTT, JSON, and DOCX writes files to the chosen folder.
- Quit and relaunch preserves jobs, models, glossary entries, telemetry choice, and license state.
- Uninstall removes application binaries.

Current Windows data paths:

```text
Database: %APPDATA%\com.audraflow.app\audraflow.db
Models:   %APPDATA%\com.audraflow.app\models
Runtime:  %APPDATA%\com.audraflow.app\runtime\components\{whisper,ffmpeg}\bin
```

After a successful NSIS install (or first app launch on MSI), Runtime Health should show Whisper and FFmpeg as ready without a manual Settings download when those tools were staged into the installer.

## Acceptance

Do not mark the Windows build as releasable until:

- `npm run release:verify:windows` passes.
- At least one NSIS install and uninstall cycle passes.
- At least one MSI install and uninstall cycle passes, if MSI is shipped.
- One fresh Windows user profile has completed a first-run transcription.
- Runtime Health has no required blockers after install, or the repair button clears the blocker.
- One upgraded profile with existing data has launched without data loss.
