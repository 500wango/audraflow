# Release Handoff Checklist

Use this checklist before sending an AudraFlow build to testers or customers.

## Build Validation

Run on every release candidate:

```bash
npm run lint
npm run build
cargo test --workspace
cargo clippy -p app --all-targets -- -D warnings
```

Linux release candidate:

```bash
npm run desktop:build:linux
npm run release:manifest -- --require-linux
npm run release:manifest:verify -- --require-linux
```

Windows release candidate, on Windows:

```powershell
npm run release:verify:windows -- -BuildInstallers
npm run release:manifest -- --require-windows
npm run release:manifest:verify -- --require-windows
```

## Artifact Handoff

Ship the installer or portable archive together with:

- `release\AudraFlow_<version>_manifest.json`
- `release\SHA256SUMS`

The manifest records artifact names, sizes, SHA256 hashes, version, generation time, and generation platform.

## Manual QA

Before marking a build as releasable:

- Install from a fresh profile and complete first-run telemetry choice.
- Confirm the bundled `base` model is present and selected on first launch.
- Import or download an additional model.
- Open Settings and confirm Runtime Health has no required blockers for the tested workflow.
- For any repairable Runtime Health warning, use the repair button and rerun the health check.
- Transcribe one short English file.
- Transcribe one short Chinese file.
- Import one direct media URL.
- Import one platform URL using the bundled `yt-dlp`.
- Export TXT, Markdown, SRT, VTT, JSON, and DOCX.
- Restart the app and confirm jobs, glossary, model selection, telemetry choice, and license state persist.
- Uninstall and confirm application binaries are removed.

## Known External Gates

- Windows signing certificate and installer signing.
- Windows NSIS/MSI verification on real hardware.
- Online license backend, if commercial activation is required.
- Large-sample QA for long audio, noisy audio, multi-speaker audio, and batch jobs.
- Music / lyrics quality tuning remains a late-stage validation track and should not block the speech-focused release candidate.
