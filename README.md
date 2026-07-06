# AudraFlow

Local transcription app built with Tauri, React, Rust, SQLite, SenseVoice, and whisper.cpp.

## Development

```bash
npm install
cargo build --workspace
npm run build
npm run lint
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Desktop Builds

The Tauri build packages the React UI plus the two required local sidecars:

- `audraflow-orchestrator`
- `audraflow-asr-runtime`

Linux packages also include the local runtime tools needed for transcription and media handling:

- `whisper-cli`
- `ffmpeg`
- `ffprobe`
- whisper.cpp shared libraries required by `whisper-cli`

Before bundling, `npm run stage:sidecars` builds the Rust sidecars in release mode and copies required external tools to `src-tauri/binaries` using Tauri's target-triple naming convention.

Build installers for the current platform:

```bash
npm run desktop:build
```

Linux packages:

```bash
npm run desktop:build:linux
```

Windows installers, run on Windows:

```powershell
npm run desktop:build:windows
```

Windows release verification, run on Windows:

```powershell
npm run release:verify:windows
```

Generate release manifest and checksums:

```bash
npm run release:manifest
npm run release:manifest:verify
```

See `docs/Windows_Release_Validation.md` for the full Windows install, smoke-test, and release checklist.
See `docs/Linux_Release_Validation.md` for Linux `.deb`, `.rpm`, AppImage install, smoke-test, and runtime troubleshooting.
See `docs/Release_Handoff_Checklist.md` for the cross-platform release handoff checklist.

For cross-target staging, set `AUDRAFLOW_TARGET_TRIPLE` or `CARGO_BUILD_TARGET` before running `npm run stage:sidecars`.

## Local ASR Engines

AudraFlow defaults to Auto engine selection for new transcription jobs. Auto uses SenseVoice for ordinary speech. In Music / lyrics mode it uses the Whisper model selected in Settings with long-context chunking. If no Whisper model is selected, it falls back to `small` for balanced music mode or `large-v3-turbo` / `large` for Music / lyrics mode with Extreme accuracy. Extreme lyrics mode also merges original-audio and Demucs-vocals candidates when possible. Whisper is slower on CPU for some files, but it gives better line-level timestamps and better lyric results than SenseVoice on music-heavy audio.

The Import page also has an audio language selector:

- `Auto detect`: best default for mixed workflows and non-Chinese media.
- `Chinese`: force `zh` for Mandarin/Chinese recordings.
- `English`: force `en` for English speech or songs.

SenseVoice is executed through Python and FunASR. Install the local dependencies before using the SenseVoice engine:

```bash
python3 -m pip install --user -U funasr modelscope
```

The first SenseVoice transcription downloads the default model from ModelScope:

```text
iic/SenseVoiceSmall
```

Optional SenseVoice environment variables:

```bash
AUDRAFLOW_PYTHON_BIN=/path/to/python
AUDRAFLOW_SENSEVOICE_MODEL=iic/SenseVoiceSmall
AUDRAFLOW_SENSEVOICE_VAD_MODEL=fsmn-vad
AUDRAFLOW_SENSEVOICE_DEVICE=cpu
```

Set `AUDRAFLOW_SENSEVOICE_DEVICE=cuda:0` only when the selected Python environment has a CUDA-capable PyTorch install.

Fun-ASR Nano is available as an experimental comparison engine. It is not used by Auto mode. The runtime expects a local `llama-funasr-cli` binary plus the GGUF model files:

```text
funasr-encoder-f16.gguf
qwen3-0.6b-q5km.gguf   # preferred, q8_0 or q4km also work
fsmn-vad.gguf          # optional for speech mode
```

By default AudraFlow searches the app data model directory (`~/.local/share/com.audraflow.app/models/funasr-nano` on Linux) plus `external/funasr-llamacpp/gguf`, `funasr-gguf`, `gguf`, and `models/funasr-nano` under the app/current working directory. You can override paths explicitly:

```bash
AUDRAFLOW_FUNASR_CLI=/path/to/llama-funasr-cli
AUDRAFLOW_FUNASR_MODEL_DIR=/path/to/gguf
# or:
AUDRAFLOW_FUNASR_ENCODER=/path/to/funasr-encoder-f16.gguf
AUDRAFLOW_FUNASR_LLM=/path/to/qwen3-0.6b-q5km.gguf
AUDRAFLOW_FUNASR_VAD=/path/to/fsmn-vad.gguf
```

For Music / lyrics mode, Fun-ASR uses fixed 15-second chunks and returns chunk-level timestamps. It is useful for fast comparison, but current validation on English lyrics still shows more wrong words than the selected Whisper large-v3-turbo path.

## Local Whisper Model

The app reads installed models from:

```text
Linux:   ~/.local/share/com.audraflow.app/models
Windows: %APPDATA%\com.audraflow.app\models
```

The current local smoke setup uses:

```text
Linux:   ~/.local/share/com.audraflow.app/models/tiny-vlocal/model.bin
Windows: %APPDATA%\com.audraflow.app\models\tiny-vlocal\model.bin
```

The Whisper runtime and orchestrator auto-discover `whisper-cli` from:

1. `AUDRAFLOW_WHISPER_CLI` or legacy `FT_WHISPER_CLI`
2. bundled desktop package tools, when installed from an AudraFlow package
3. `external/whisper.cpp/build-linux/bin/whisper-cli` or `external/whisper.cpp/build/bin/whisper-cli` on Linux
4. `external\whisper.cpp\build\bin\whisper-cli.exe` on Windows
5. `PATH`

Linux packages include bundled `ffmpeg`, `ffprobe`, and `whisper-cli` for local files. Platform page URLs such as YouTube-style links require `yt-dlp` in `PATH`, a bundled `yt-dlp` next to the app, or explicit `AUDRAFLOW_YT_DLP_BIN`.

## Music / Lyrics Mode

Music transcription can optionally run vocal separation before ASR:

```text
original media -> Demucs vocals.wav -> whisper.cpp -> AudraFlow editor/export
```

Install Demucs with the audio save dependency required by current torchaudio releases:

```bash
python3 -m pip install --user -U demucs torchcodec
```

The desktop UI shows the vocal separation option only when Music / lyrics mode is enabled. In normal music mode, vocal separation uses the Demucs `vocals.wav` output directly. In Music / lyrics mode with Extreme accuracy enabled, AudraFlow runs two Whisper candidates when possible: original audio and Demucs-isolated vocals. It then uses the vocals candidate as the primary source when it is competitive and fills gaps from the original-audio candidate.

AudraFlow auto-detects `demucs`, `python3 -m demucs`, `python -m demucs`, or `py -3 -m demucs`. To force a specific executable, set:

```bash
AUDRAFLOW_DEMUCS_BIN=/path/to/demucs
```

If Demucs is unavailable or fails, AudraFlow falls back to the original audio and continues transcription.

Lyrics mode filters common credit, instrumental marker, and video-watermark hallucinations after decoding. Advanced users can opt into whisper.cpp prompt or token-regex controls with `AUDRAFLOW_LYRICS_PROMPT` and `AUDRAFLOW_LYRICS_SUPPRESS_REGEX`, but AudraFlow does not inject default lyrics prompts because they can leak into sung-audio transcripts.

## End-to-End Smoke Test

Run the real IPC queue and transcription pipeline:

```bash
npm run smoke:e2e
```

On Linux this starts `target/debug/audraflow-orchestrator`, sends a `JobCreate` message over a Unix socket, waits for completion, verifies transcript segments in SQLite, and removes the generated `ipc-smoke-*` job unless `--keep-record` is passed.

On Windows the same npm script dispatches to the existing PowerShell smoke test and uses the Windows Named Pipe transport.

Optional direct script usage:

```bash
npm run smoke:e2e -- --skip-build
npm run smoke:e2e -- --audio-path path/to/audio.mp3 --model-path path/to/model.bin
```
