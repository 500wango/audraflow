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

The Tauri build packages the React UI, the two required local sidecars, and the runtime tools needed to run after the user downloads or imports an ASR model.

- `audraflow-orchestrator`
- `audraflow-asr-runtime`

Linux packages also include the local runtime tools needed for transcription and media handling:

- `whisper-cli`
- `ffmpeg`
- `ffprobe`
- `yt-dlp`
- whisper.cpp shared libraries required by `whisper-cli`

Windows and macOS packages include the same command-line tools without the Linux shared libraries. Windows packages also include a bundled Python runtime with Demucs and SenseVoice Python dependencies.

Before bundling, `npm run prepare:runtime-assets` fetches the platform `yt-dlp` binary, then `npm run stage:sidecars` builds the Rust sidecars in release mode and copies required external tools to `src-tauri/binaries` using Tauri's target-triple naming convention. Windows builds also run `npm run prepare:windows-runtime` to build the bundled Python/Demucs/SenseVoice runtime.

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

AudraFlow defaults to Auto engine selection for new transcription jobs. Release packages include the native runtime tools, but ASR models are downloaded or imported from Settings. Auto uses local Whisper after a Whisper model is selected. Windows packages include the Python dependencies for SenseVoice and Demucs; SenseVoice model files are still downloaded by the engine on first use. In Music / lyrics mode, Auto uses the selected or preferred Whisper model with long-context chunking. Extreme lyrics mode also merges original-audio and Demucs-vocals candidates when possible.

Settings includes Runtime Health for install validation and one-click repair. It can download the default Whisper model, install a managed `yt-dlp`, and run Python package repair as a fallback for optional SenseVoice and Demucs support. Manual commands and environment variables below are fallback paths when the bundled runtime or repair action cannot be used in the target environment.

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

Release packages include bundled `ffmpeg`, `ffprobe`, `whisper-cli`, and `yt-dlp` for local files and platform page URLs such as YouTube-style links. You can still override them with `AUDRAFLOW_FFMPEG_BIN`, `AUDRAFLOW_FFPROBE_BIN`, `AUDRAFLOW_WHISPER_CLI`, or `AUDRAFLOW_YT_DLP_BIN`.

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
