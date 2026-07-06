# Linux Release Validation

Run this checklist on a Linux x64 desktop before handing AudraFlow to testers.

## Prerequisites

- Ubuntu/Debian-compatible system for `.deb`, Fedora/RHEL-compatible system for `.rpm`, or any supported desktop distribution for AppImage.
- GTK 3 and WebKitGTK 4.1 runtime libraries available on the target desktop.
- A local Whisper model for Whisper workflows, or SenseVoice Python dependencies for the default speech workflow.
- Optional `yt-dlp` for YouTube, Bilibili, and other platform links.
- Optional `demucs` and `torchcodec` for vocal separation in Music / lyrics mode.

## Build

From the repository root:

```bash
npm install
npm run lint
npm run build
cargo test --workspace
cargo clippy -p app --all-targets -- -D warnings
npm run desktop:build:linux
npm run release:manifest -- --require-linux
npm run release:manifest:verify -- --require-linux
```

The Linux artifacts are expected under:

```text
target/release/bundle/deb/AudraFlow_1.0.0_amd64.deb
target/release/bundle/rpm/AudraFlow-1.0.0-1.x86_64.rpm
target/release/bundle/appimage/AudraFlow_1.0.0_amd64.AppImage
```

## Install Check

Debian or Ubuntu:

```bash
sudo dpkg -i target/release/bundle/deb/AudraFlow_1.0.0_amd64.deb
```

Fedora, RHEL, or compatible systems:

```bash
sudo rpm -Uvh target/release/bundle/rpm/AudraFlow-1.0.0-1.x86_64.rpm
```

AppImage:

```bash
chmod +x target/release/bundle/appimage/AudraFlow_1.0.0_amd64.AppImage
target/release/bundle/appimage/AudraFlow_1.0.0_amd64.AppImage
```

If `apt` prints a sandbox warning about `_apt` permissions while installing a local `.deb`, the install can still be valid. It means the local package file is not readable by the `_apt` sandbox user, so apt/dpkg used elevated access for that local file.

## Runtime Health Check

After launch, open Settings and verify Runtime Health:

- `Whisper CLI`, `FFmpeg`, and `FFprobe` are ready for local Whisper and media decoding.
- `SenseVoice Python packages` are ready if the default SenseVoice speech engine will be used.
- `yt-dlp` is ready if platform links will be imported.
- `Demucs` is ready only if vocal separation will be used.
- `Fun-ASR CLI` and `Fun-ASR GGUF models` are only required for the experimental Fun-ASR engine.

The Import page should block new jobs before creation when required runtime dependencies are missing.

## Package Smoke Test Without Installing

To verify the `.deb` contents without sudo:

```bash
rm -rf /tmp/audraflow-deb-smoke
mkdir -p /tmp/audraflow-deb-smoke
dpkg-deb -x target/release/bundle/deb/AudraFlow_1.0.0_amd64.deb /tmp/audraflow-deb-smoke

LD_LIBRARY_PATH=/tmp/audraflow-deb-smoke/usr/bin \
  /tmp/audraflow-deb-smoke/usr/bin/whisper-cli \
  -m ~/.local/share/com.audraflow.app/models/base-vwhisper.cpp-5359861c739e955e79d9a303bcbc70fb988958b1/model.bin \
  -f external/whisper.cpp/samples/jfk.wav \
  -otxt \
  -of /tmp/audraflow-deb-smoke/jfk-smoke
```

The transcript should contain:

```text
And so my fellow Americans ask not what your country can do for you
```

## Common Runtime Fixes

Platform links fail with `yt-dlp` missing:

```bash
python3 -m pip install --user -U yt-dlp
# or set:
AUDRAFLOW_YT_DLP_BIN=/path/to/yt-dlp
```

SenseVoice dependencies missing:

```bash
python3 -m pip install --user -U funasr modelscope
```

Demucs vocal separation missing:

```bash
python3 -m pip install --user -U demucs torchcodec
```

If the desktop save dialog reports a Tauri ACL error, rebuild and reinstall the current package. That indicates the installed application is older than the current permissions configuration.

## Manual QA

Before marking the Linux build releasable:

- Launch from the desktop menu or installed executable.
- Complete the first-run telemetry choice with both choices tested on fresh profiles.
- Switch the UI language between English and Chinese.
- Import or download a Whisper model and confirm Settings shows it selected.
- Import one local short English file and one local short Chinese file.
- Import one direct media URL.
- Import one platform URL with `yt-dlp` installed.
- Export TXT, Markdown, SRT, VTT, JSON, and DOCX to a writable folder.
- Restart the app and confirm jobs, model selection, glossary entries, telemetry choice, and license state persist.
- Uninstall and confirm application binaries are removed.

## Acceptance

Do not mark the Linux build as releasable until:

- `npm run release:manifest:verify -- --require-linux` passes.
- At least one `.deb` install and launch cycle passes on a clean profile.
- At least one AppImage launch passes on a clean profile.
- Runtime Health shows no required blockers for the tested workflow.
- One short real transcription completes and exports successfully.
