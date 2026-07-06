//! AudraFlow Model Manager
//!
//! Handles model download, SHA256 verification, version management,
//! cache cleanup, and manual model directory configuration.
//! Supports offline use; download failures are recoverable.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A managed ASR model version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub name: String,
    pub version: String,
    pub language: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub download_url: String,
    pub model_type: ModelType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelType {
    WhisperCpp,
    DiarizationVad,
    Punctuation,
}

/// A model installed into the local cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledModel {
    pub info: ModelInfo,
    pub path: PathBuf,
    pub installed_at_ms: i64,
}

/// The user's currently selected model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSelection {
    pub name: String,
    pub version: String,
}

/// A local or remote manifest of known model versions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelManifest {
    pub models: Vec<ModelInfo>,
}

/// The model manager.
pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// Create a new model manager with the given models directory.
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Ensure the models directory exists.
    pub fn init(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.models_dir)?;
        Ok(())
    }

    /// Return the model cache root.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Check if a model is downloaded and valid.
    pub fn is_model_available(&self, model: &ModelInfo) -> bool {
        let path = self.model_path(model);
        path.exists() && self.verify_checksum(model).is_ok()
    }

    /// Get the path where a model will be / is stored.
    pub fn model_path(&self, model: &ModelInfo) -> PathBuf {
        self.model_dir(model).join("model.bin")
    }

    /// Download a model with progress callback. Returns bytes written.
    pub fn download<F>(&self, model: &ModelInfo, progress: F) -> anyhow::Result<u64>
    where
        F: Fn(u64, u64), // (downloaded_bytes, total_bytes)
    {
        let dir = self.model_dir(model);
        std::fs::create_dir_all(&dir)?;

        let output_path = dir.join("model.bin");
        let tmp_path = dir.join("model.bin.tmp");

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()?;

        let existing_bytes = std::fs::metadata(&tmp_path)
            .map(|meta| meta.len())
            .unwrap_or(0)
            .min(model.size_bytes);
        let mut request = client.get(&model.download_url);
        if existing_bytes > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing_bytes}-"));
        }

        let mut resp = request.send()?;
        let accepts_resume = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let total = resp
            .content_length()
            .map(|remaining| remaining + if accepts_resume { existing_bytes } else { 0 })
            .unwrap_or(model.size_bytes);

        let mut file = if accepts_resume && existing_bytes > 0 {
            std::fs::OpenOptions::new().append(true).open(&tmp_path)?
        } else {
            std::fs::File::create(&tmp_path)?
        };
        let mut downloaded: u64 = if accepts_resume { existing_bytes } else { 0 };
        if downloaded > 0 {
            progress(downloaded, total);
        }
        let mut buf = [0u8; 8192];

        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            downloaded += n as u64;
            progress(downloaded, total);
        }

        // Verify checksum before renaming
        drop(file);
        let actual_hash = self.hash_file(&tmp_path)?;
        if actual_hash != model.sha256 {
            std::fs::remove_file(&tmp_path)?;
            anyhow::bail!(
                "SHA256 mismatch: expected {}, got {}",
                model.sha256,
                actual_hash
            );
        }

        std::fs::rename(&tmp_path, &output_path)?;
        self.write_installed_metadata(model)?;
        Ok(downloaded)
    }

    /// Verify the SHA256 checksum of a downloaded model.
    pub fn verify_checksum(&self, model: &ModelInfo) -> anyhow::Result<()> {
        let path = self.model_path(model);
        if !path.exists() {
            anyhow::bail!("Model file not found: {}", path.display());
        }
        let hash = self.hash_file(&path)?;
        if hash != model.sha256 {
            anyhow::bail!("SHA256 mismatch: expected {}, got {}", model.sha256, hash);
        }
        Ok(())
    }

    /// Register an already-copied local model after checksum validation.
    pub fn register_installed_model(&self, model: &ModelInfo) -> anyhow::Result<InstalledModel> {
        self.verify_checksum(model)?;
        self.write_installed_metadata(model)?;
        read_json(&self.model_dir(model).join("model.json"))
    }

    /// List all downloaded models.
    pub fn list_models(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(self
            .list_installed_models()?
            .into_iter()
            .map(|model| model.path)
            .collect())
    }

    /// List all downloaded models with metadata.
    pub fn list_installed_models(&self) -> anyhow::Result<Vec<InstalledModel>> {
        let mut models = Vec::new();
        if !self.models_dir.exists() {
            return Ok(models);
        }

        for entry in std::fs::read_dir(&self.models_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let model_file = entry.path().join("model.bin");
            let metadata_file = entry.path().join("model.json");
            if !model_file.exists() || !metadata_file.exists() {
                continue;
            }

            if let Ok(installed) = read_json::<InstalledModel>(&metadata_file) {
                models.push(installed);
            }
        }

        models.sort_by(|a, b| {
            a.info
                .name
                .cmp(&b.info.name)
                .then_with(|| a.info.version.cmp(&b.info.version))
        });
        Ok(models)
    }

    /// Get total cache size in bytes.
    pub fn cache_size_bytes(&self) -> anyhow::Result<u64> {
        let mut total = 0u64;
        for model_path in self.list_models()? {
            total += std::fs::metadata(&model_path)?.len();
        }
        Ok(total)
    }

    /// Remove a specific model from cache.
    pub fn remove_model(&self, name: &str, version: &str) -> anyhow::Result<()> {
        let dir = self.models_dir.join(format!("{}-v{}", name, version));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        if self.selected_model()?.is_some_and(|selection| {
            selection.info.name == name && selection.info.version == version
        }) {
            let selection_path = self.selection_path();
            if selection_path.exists() {
                std::fs::remove_file(selection_path)?;
            }
        }
        Ok(())
    }

    /// Clear all model cache.
    pub fn clear_all(&self) -> anyhow::Result<()> {
        if self.models_dir.exists() {
            for entry in std::fs::read_dir(&self.models_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    std::fs::remove_dir_all(entry.path())?;
                } else if entry.file_name() == "selected-model.json" {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(())
    }

    // ── Internal ────────────────────────────────────────────────────────────

    /// Select an installed model version for future transcription.
    pub fn select_model(&self, name: &str, version: &str) -> anyhow::Result<InstalledModel> {
        let installed = self
            .list_installed_models()?
            .into_iter()
            .find(|model| model.info.name == name && model.info.version == version)
            .ok_or_else(|| anyhow::anyhow!("Model is not installed: {name} v{version}"))?;

        let selection = ModelSelection {
            name: name.into(),
            version: version.into(),
        };
        write_json(&self.selection_path(), &selection)?;
        Ok(installed)
    }

    /// Return the selected model metadata if it is installed.
    pub fn selected_model(&self) -> anyhow::Result<Option<InstalledModel>> {
        let selection_path = self.selection_path();
        if !selection_path.exists() {
            return Ok(None);
        }

        let selection = read_json::<ModelSelection>(&selection_path)?;
        let selected = self.list_installed_models()?.into_iter().find(|model| {
            model.info.name == selection.name && model.info.version == selection.version
        });

        if selected.is_none() {
            std::fs::remove_file(selection_path)?;
        }

        Ok(selected)
    }

    /// Resolve the active model path for ASR startup.
    pub fn selected_model_path(&self) -> anyhow::Result<Option<PathBuf>> {
        Ok(self.selected_model()?.map(|model| model.path))
    }

    /// Read a model manifest from a JSON file.
    pub fn load_manifest(path: &Path) -> anyhow::Result<ModelManifest> {
        read_json(path)
    }

    /// Write a model manifest to a JSON file.
    pub fn save_manifest(path: &Path, manifest: &ModelManifest) -> anyhow::Result<()> {
        write_json(path, manifest)
    }

    fn model_dir(&self, model: &ModelInfo) -> PathBuf {
        self.models_dir
            .join(format!("{}-v{}", model.name, model.version))
    }

    fn selection_path(&self) -> PathBuf {
        self.models_dir.join("selected-model.json")
    }

    fn write_installed_metadata(&self, model: &ModelInfo) -> anyhow::Result<()> {
        let installed = InstalledModel {
            info: model.clone(),
            path: self.model_path(model),
            installed_at_ms: now_unix_ms(),
        };
        write_json(&self.model_dir(model).join("model.json"), &installed)
    }

    fn hash_file(&self, path: &Path) -> anyhow::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(format!("{:x}", hasher.finalize()))
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model(name: &str, version: &str) -> ModelInfo {
        ModelInfo {
            name: name.into(),
            version: version.into(),
            language: "zh".into(),
            size_bytes: 5,
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
            download_url: "https://example.com/model.bin".into(),
            model_type: ModelType::WhisperCpp,
        }
    }

    fn install_fake_model(mgr: &ModelManager, model: &ModelInfo) {
        let path = mgr.model_path(model);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"hello").unwrap();
        mgr.write_installed_metadata(model).unwrap();
    }

    #[test]
    fn test_model_path() {
        let mgr = ModelManager::new(PathBuf::from("/tmp/models"));
        let info = sample_model("whisper-base", "1.0.0");
        let path = mgr.model_path(&info);
        assert!(path.to_string_lossy().contains("whisper-base-v1.0.0"));
    }

    #[test]
    fn installed_models_are_listed_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().join("models"));
        let base = sample_model("whisper-base", "1.0.0");
        let small = sample_model("whisper-small", "1.0.0");
        install_fake_model(&mgr, &small);
        install_fake_model(&mgr, &base);

        let installed = mgr.list_installed_models().unwrap();

        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0].info.name, "whisper-base");
        assert_eq!(installed[1].info.name, "whisper-small");
        assert!(mgr.list_models().unwrap()[0].ends_with("model.bin"));
    }

    #[test]
    fn selected_model_roundtrips_and_clears_when_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().join("models"));
        let model = sample_model("whisper-base", "1.0.0");
        install_fake_model(&mgr, &model);

        let selected = mgr.select_model("whisper-base", "1.0.0").unwrap();
        assert_eq!(selected.info, model);
        assert_eq!(
            mgr.selected_model_path().unwrap(),
            Some(mgr.model_path(&model))
        );

        mgr.remove_model("whisper-base", "1.0.0").unwrap();

        assert!(mgr.selected_model().unwrap().is_none());
        assert!(mgr.selected_model_path().unwrap().is_none());
    }

    #[test]
    fn remove_model_keeps_other_versions_and_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().join("models"));
        let base = sample_model("whisper-base", "1.0.0");
        let small = sample_model("whisper-small", "1.0.0");
        install_fake_model(&mgr, &base);
        install_fake_model(&mgr, &small);
        mgr.select_model("whisper-base", "1.0.0").unwrap();

        mgr.remove_model("whisper-small", "1.0.0").unwrap();
        let installed = mgr.list_installed_models().unwrap();

        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].info.name, "whisper-base");
        assert_eq!(
            mgr.selected_model().unwrap().unwrap().info.name,
            "whisper-base"
        );
    }

    #[test]
    fn manifest_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let manifest = ModelManifest {
            models: vec![sample_model("whisper-base", "1.0.0")],
        };

        ModelManager::save_manifest(&path, &manifest).unwrap();
        let loaded = ModelManager::load_manifest(&path).unwrap();

        assert_eq!(loaded, manifest);
    }
}
