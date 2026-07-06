# Windows Release Validation

Run this checklist on a real Windows x64 machine before handing AudraFlow to testers.

## Prerequisites

- Windows 10 or 11 x64.
- Node.js 20.19+ or 22.12+ and npm 10+.
- Rust stable with the MSVC toolchain.
- Visual Studio Build Tools with the C++ desktop workload.
- PowerShell 5.1+.
- A local Whisper model, preferably `external\whisper.cpp\models\ggml-tiny.bin`.
- Bundled Windows runtime tools in `release\windows-portable\AudraFlow\bin`.

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
- Model import or model download succeeds.
- A short English audio file transcribes successfully.
- A short Chinese audio file transcribes successfully.
- Search finds transcript text.
- Glossary add, edit, and delete work.
- Export to TXT, Markdown, SRT, VTT, JSON, and DOCX writes files to the chosen folder.
- Quit and relaunch preserves jobs, models, glossary entries, telemetry choice, and license state.
- Uninstall removes application binaries.

Current Windows data paths:

```text
Database: %APPDATA%\AudraFlow\audraflow.db
Models:   Tauri app data directory\models
```

## Acceptance

Do not mark the Windows build as releasable until:

- `npm run release:verify:windows` passes.
- At least one NSIS install and uninstall cycle passes.
- At least one MSI install and uninstall cycle passes, if MSI is shipped.
- One fresh Windows user profile has completed a first-run transcription.
- One upgraded profile with existing data has launched without data loss.
