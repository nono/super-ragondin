use crate::error::Result;
use crate::remote::auth::OAuthClient;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub instance_url: String,
    pub sync_dir: PathBuf,
    pub data_dir: PathBuf,
    pub oauth_client: Option<OAuthClient>,
    pub last_seq: Option<String>,
}

impl Config {
    /// Load configuration from a file.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if reading or parsing the file fails.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(Some(config))
    }

    /// Save configuration to a file.
    ///
    /// Creates parent directories if they don't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if writing the file fails.
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.data_dir.join("staging")
    }

    #[must_use]
    pub fn store_dir(&self) -> PathBuf {
        self.data_dir.join("store")
    }
}
