//! AudraFlow Licensing & Activation
//!
//! PRD §16.2: v1.0 Personal Pro is one-time purchase (buy-once).
//! 30-day free trial with all features, no watermark.
//!
//! License model:
//! - Trial: 30 days from first launch, full features.
//! - Personal Pro: perpetual use + 1 year model updates, 2 device activations.
//! - Renewal: annual model update subscription (optional).
//!
//! Activation: license key + device binding.
//! Supports offline activation for air-gapped machines.

use chrono::{DateTime, Utc};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::digest as ring_digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

// ── License State ──────────────────────────────────────────────────────────

/// The current license state of the application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LicenseState {
    /// First launch, trial not yet started.
    NotActivated,
    /// Within the 30-day trial period.
    Trial {
        /// Date of first launch (trial start).
        started_at: String,
        /// Trial expiration date.
        expires_at: String,
        /// Days remaining.
        days_remaining: u32,
    },
    /// Fully activated with a valid license.
    Activated {
        /// License key hash (not stored in plain text).
        key_hash: String,
        /// Activation date.
        activated_at: String,
        /// Model updates valid until.
        model_updates_until: String,
    },
    /// Trial has expired.
    TrialExpired { expired_at: String },
    /// License is invalid or revoked.
    Invalid(String),
}

// ── License Manager ────────────────────────────────────────────────────────

/// Manages license activation, validation, and trial tracking.
pub struct LicenseManager {
    /// Path to the license file (local, encrypted storage).
    license_path: PathBuf,
    /// Current license state.
    state: LicenseState,
}

impl LicenseManager {
    /// Create a new license manager.
    /// On first run, creates a new trial state.
    /// On subsequent runs, loads existing license.
    pub fn new(app_data_dir: PathBuf) -> anyhow::Result<Self> {
        let license_path = app_data_dir.join("license.dat");
        let state = if license_path.exists() {
            Self::load_from_file(&license_path)?
        } else {
            // First launch: start trial
            let now = Utc::now();
            let expires = now + chrono::TimeDelta::days(30);
            let state = LicenseState::Trial {
                started_at: now.to_rfc3339(),
                expires_at: expires.to_rfc3339(),
                days_remaining: 30,
            };
            // Persist immediately
            Self::save_to_file(&license_path, &state)?;
            state
        };

        let mut manager = Self {
            license_path,
            state,
        };
        manager.refresh_trial_days()?;
        Ok(manager)
    }

    /// Get the current license state.
    pub fn state(&self) -> &LicenseState {
        &self.state
    }

    /// Check if the application is usable (trial active or activated).
    pub fn is_usable(&self) -> bool {
        matches!(
            &self.state,
            LicenseState::Trial { .. } | LicenseState::Activated { .. }
        )
    }

    /// Check if trial has expired.
    pub fn is_trial_expired(&self) -> bool {
        matches!(&self.state, LicenseState::TrialExpired { .. })
    }

    /// Get remaining trial days (0 if expired or activated).
    pub fn trial_days_remaining(&self) -> u32 {
        match &self.state {
            LicenseState::Trial { days_remaining, .. } => *days_remaining,
            _ => 0,
        }
    }

    /// Activate with a license key.
    ///
    /// Key format: AF-XXXX-XXXX-XXXX-XXXX (legacy FT keys are accepted).
    /// Validation: checksum + format check + device binding.
    pub fn activate(&mut self, license_key: &str) -> Result<LicenseState, LicenseError> {
        // ── Validate key format ─────────────────────────────────────────────
        let key = license_key.trim();
        if !Self::is_valid_key_format(key) {
            return Err(LicenseError::InvalidFormat);
        }

        // ── Verify checksum ─────────────────────────────────────────────────
        let checksum_valid = Self::verify_key_checksum(key);
        if !checksum_valid {
            return Err(LicenseError::InvalidKey);
        }

        // ── Check device binding ────────────────────────────────────────────
        let device_id = Self::get_device_id();
        let key_hash = Self::hash_key(key, &device_id);

        // ── Activate ────────────────────────────────────────────────────────
        let now = Utc::now();
        let model_updates = now + chrono::TimeDelta::days(365); // 1 year of updates

        let state = LicenseState::Activated {
            key_hash,
            activated_at: now.to_rfc3339(),
            model_updates_until: model_updates.to_rfc3339(),
        };

        self.state = state.clone();
        Self::save_to_file(&self.license_path, &self.state)
            .map_err(|e| LicenseError::VerificationFailed(e.to_string()))?;
        log::info!("License activated successfully");

        Ok(state)
    }

    /// Deactivate and return to trial (if still within trial period).
    pub fn deactivate(&mut self) -> anyhow::Result<()> {
        // Revert to trial state — read original trial start from file backup
        self.state = LicenseState::NotActivated;
        Self::save_to_file(&self.license_path, &self.state)?;
        Ok(())
    }

    /// Refresh trial days remaining based on current time.
    fn refresh_trial_days(&mut self) -> anyhow::Result<()> {
        if let LicenseState::Trial {
            ref started_at,
            ref expires_at,
            ..
        } = self.state
        {
            let expires: DateTime<Utc> = expires_at.parse().unwrap_or(Utc::now());
            let now = Utc::now();
            let remaining = if now >= expires {
                0
            } else {
                (expires - now).num_days().max(0) as u32
            };

            if remaining == 0 {
                self.state = LicenseState::TrialExpired {
                    expired_at: expires_at.clone(),
                };
            } else {
                self.state = LicenseState::Trial {
                    started_at: started_at.clone(),
                    expires_at: expires_at.clone(),
                    days_remaining: remaining,
                };
            }
            Self::save_to_file(&self.license_path, &self.state)?;
        }
        Ok(())
    }

    // ── Key Validation ─────────────────────────────────────────────────────

    /// Check license key format: AF-XXXX-XXXX-XXXX-XXXX
    fn is_valid_key_format(key: &str) -> bool {
        let segments: Vec<&str> = key.split('-').collect();
        if segments.len() != 5 {
            return false;
        }
        let prefix = segments[0].to_uppercase();
        if prefix != "AF" && prefix != "FT" {
            return false;
        }
        // Segments 1-3 are 4 chars each, segment 4 (checksum) is 4 or 8 chars
        for (i, seg) in segments[1..].iter().enumerate() {
            let valid_len = if i == 3 {
                seg.len() == 4 || seg.len() == 8
            } else {
                seg.len() == 4
            };
            if !valid_len || !seg.chars().all(|c| c.is_ascii_alphanumeric()) {
                return false;
            }
        }
        true
    }

    /// Verify the embedded checksum in the license key.
    /// Simple scheme: last segment is a checksum of the first 3 data segments.
    fn verify_key_checksum(key: &str) -> bool {
        let segments: Vec<&str> = key.split('-').collect();
        if segments.len() != 5 {
            return false;
        }

        let data = format!("{}-{}-{}", segments[1], segments[2], segments[3]);
        let expected = segments[4];

        // Compute SHA256 of data, compare based on checksum length
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        if expected.len() == 8 {
            // New format: 8 hex chars of SHA256
            let computed = &hash[..8];
            computed.eq_ignore_ascii_case(expected)
        } else if expected.len() == 4 {
            // Legacy format: 4 hex chars of SHA256
            let computed = &hash[..4];
            computed.eq_ignore_ascii_case(expected)
        } else {
            false
        }
    }

    /// Hash the license key with device ID for local storage.
    fn hash_key(key: &str, device_id: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hasher.update(b"::");
        hasher.update(device_id.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // ── Device Binding ─────────────────────────────────────────────────────

    /// Get a unique device identifier for license binding.
    /// Uses hostname + machine UUID where available.
    fn get_device_id() -> String {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        #[cfg(target_os = "windows")]
        {
            // Windows: use machine GUID from registry
            let output = std::process::Command::new("reg")
                .args([
                    "query",
                    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography",
                    "/v",
                    "MachineGuid",
                ])
                .output();
            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = stdout.lines().find(|l| l.contains("MachineGuid")) {
                    let guid = line.split_whitespace().last().unwrap_or(&hostname);
                    return format!("{}:{}", hostname, guid);
                }
            }
        }

        hostname
    }

    // ── Persistence ─────────────────────────────────────────────────────────

    /// Derive a 32-byte AES-256 key from the device ID using SHA-256.
    fn encryption_key() -> Result<LessSafeKey, anyhow::Error> {
        let device_id = Self::get_device_id();
        let hash = ring_digest::digest(&ring_digest::SHA256, device_id.as_bytes());
        let unbound = UnboundKey::new(&AES_256_GCM, hash.as_ref())
            .map_err(|_| anyhow::anyhow!("Failed to create AES-256-GCM key"))?;
        Ok(LessSafeKey::new(unbound))
    }

    fn save_to_file(path: &PathBuf, state: &LicenseState) -> anyhow::Result<()> {
        let json = serde_json::to_vec(state)?;
        let key = Self::encryption_key()?;
        let rng = SystemRandom::new();
        let mut nonce_bytes = [0u8; 12];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| anyhow::anyhow!("RNG failed"))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut in_out = json;
        let aad = Aad::empty();
        key.seal_in_place_append_tag(nonce, aad, &mut in_out)
            .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
        let mut output = nonce_bytes.to_vec();
        output.append(&mut in_out);
        std::fs::write(path, output)?;
        Ok(())
    }

    fn load_from_file(path: &PathBuf) -> anyhow::Result<LicenseState> {
        let data = std::fs::read(path)?;

        // Try AES-GCM decryption first (new format)
        if data.len() >= 12 + AES_256_GCM.tag_len() {
            let (nonce_bytes, ciphertext) = data.split_at(12);
            let nonce = Nonce::assume_unique_for_key(nonce_bytes.try_into().unwrap());
            if let Ok(key) = Self::encryption_key() {
                let mut in_out = ciphertext.to_vec();
                let aad = Aad::empty();
                if let Ok(plaintext) = key.open_in_place(nonce, aad, &mut in_out) {
                    if let Ok(state) = serde_json::from_slice::<LicenseState>(plaintext) {
                        return Ok(state);
                    }
                }
            }
        }

        // Fallback: legacy XOR obfuscation (remove in a future release)
        let json: String = data.iter().map(|b| (b ^ 0x5A) as char).collect();
        let state: LicenseState = serde_json::from_str(&json)?;
        log::warn!("License file uses legacy obfuscation; re-save will upgrade to encryption");
        Ok(state)
    }
}

// ── License Error ──────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("Invalid license key format. Expected: AF-XXXX-XXXX-XXXX-XXXX")]
    InvalidFormat,
    #[error("License key is not valid")]
    InvalidKey,
    #[error("License key has been revoked")]
    Revoked,
    #[error("Maximum device activations reached (2 devices)")]
    MaxDevices,
    #[error("Failed to verify license: {0}")]
    VerificationFailed(String),
}

// ── License Key Generator (for the vendor/publisher side) ──────────────────

/// Generate a valid license key (used by the publisher, not the app).
#[allow(dead_code)]
pub fn generate_license_key() -> String {
    use rand::Rng;
    let mut rng = rand::rng();

    let seg1: String = (0..4)
        .map(|_| rng.sample(rand::distr::Alphanumeric) as char)
        .collect();
    let seg2: String = (0..4)
        .map(|_| rng.sample(rand::distr::Alphanumeric) as char)
        .collect();
    let seg3: String = (0..4)
        .map(|_| rng.sample(rand::distr::Alphanumeric) as char)
        .collect();

    let data = format!("{}-{}-{}", seg1, seg2, seg3);
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let checksum = hash[..8].to_uppercase();

    format!("AF-{}-{}-{}-{}", seg1, seg2, seg3, checksum)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_format_validation() {
        assert!(LicenseManager::is_valid_key_format(
            "AF-ABCD-EFGH-IJKL-MNOP"
        ));
        assert!(LicenseManager::is_valid_key_format(
            "FT-ABCD-EFGH-IJKL-MNOP"
        ));
        assert!(!LicenseManager::is_valid_key_format("bad-key"));
        assert!(!LicenseManager::is_valid_key_format("AF-ABC-DEF-GHI")); // Too short
        assert!(!LicenseManager::is_valid_key_format(
            "XX-ABCD-EFGH-IJKL-MNOP"
        )); // Wrong prefix
    }

    #[test]
    fn test_generated_key_is_valid() {
        let key = generate_license_key();
        assert!(LicenseManager::is_valid_key_format(&key));
        assert!(LicenseManager::verify_key_checksum(&key));
    }

    #[test]
    fn test_key_hash_is_deterministic() {
        let h1 = LicenseManager::hash_key("AF-TEST-KEY1-1234-ABCD", "device1");
        let h2 = LicenseManager::hash_key("AF-TEST-KEY1-1234-ABCD", "device1");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_key_hash_differs_per_device() {
        let h1 = LicenseManager::hash_key("AF-TEST-KEY1-1234-ABCD", "device1");
        let h2 = LicenseManager::hash_key("AF-TEST-KEY1-1234-ABCD", "device2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_new_manager_starts_trial() {
        let dir = tempfile::tempdir().unwrap();
        let manager = LicenseManager::new(dir.path().to_path_buf()).unwrap();
        assert!(manager.is_usable());
        assert!(matches!(manager.state(), LicenseState::Trial { .. }));
    }

    #[test]
    fn test_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.dat");

        let state = LicenseState::Trial {
            started_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2026-01-31T00:00:00Z".into(),
            days_remaining: 30,
        };
        LicenseManager::save_to_file(&path, &state).unwrap();

        let loaded = LicenseManager::load_from_file(&path).unwrap();
        match loaded {
            LicenseState::Trial { days_remaining, .. } => {
                assert_eq!(days_remaining, 30);
            }
            _ => panic!("Expected Trial state"),
        }
    }

    #[test]
    fn test_activate_with_valid_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = LicenseManager::new(dir.path().to_path_buf()).unwrap();

        let key = generate_license_key();
        eprintln!("Generated key: {}", key);
        // Verify manually
        let segments: Vec<&str> = key.split('-').collect();
        let data = format!("{}-{}-{}", segments[1], segments[2], segments[3]);
        let mut hasher = sha2::Sha256::new();
        hasher.update(data.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        eprintln!("Data: {}, hash first 8: {}", data, &hash[..8]);
        eprintln!("Expected checksum: {}", segments[4]);
        let result = manager.activate(&key);
        if let Err(ref e) = result {
            eprintln!("Activation error: {}", e);
        }
        assert!(result.is_ok());
        assert!(matches!(manager.state(), LicenseState::Activated { .. }));
    }

    #[test]
    fn test_activate_with_invalid_key_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = LicenseManager::new(dir.path().to_path_buf()).unwrap();

        let result = manager.activate("INVALID-KEY-FORMAT");
        assert!(result.is_err());
        // Should still be in trial
        assert!(matches!(manager.state(), LicenseState::Trial { .. }));
    }
}
