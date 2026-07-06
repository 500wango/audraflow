#[derive(Debug)]
struct RuntimeTranscribeConfig {
    input_path: PathBuf,
    model_path: Option<PathBuf>,
    whisper_cli: PathBuf,
    language: String,
    file_hash: String,
    asr_engine: AsrEngine,
    extreme_accuracy: bool,
    audio_mode: AudioMode,
    vocal_separation: VocalSeparationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrEngine {
    Whisper,
    SenseVoice,
    FunAsr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    Speech,
    Music,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocalSeparationMode {
    Off,
    Demucs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SenseVoiceChunkingPlan {
    max_chunk_ms: i64,
    overlap_ms: i64,
    internal_vad: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTranscribeOutput {
    segments: Vec<Segment>,
    audio_duration_s: f64,
    rtf: f64,
    ttfv_s: f64,
    chunk_count: u32,
    preprocess_messages: Vec<String>,
}
