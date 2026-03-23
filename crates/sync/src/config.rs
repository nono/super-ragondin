use crate::error::Result;
use crate::remote::auth::OAuthClient;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub instance_url: String,
    pub sync_dir: PathBuf,
    pub data_dir: PathBuf,
    pub oauth_client: Option<OAuthClient>,
    pub last_seq: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
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
        tracing::debug!(path = %path.display(), "⚙️ Loading config");
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
        tracing::debug!(path = %path.display(), "⚙️ Saving config");
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, content)?;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Validate that the sync directory is not a system root or the home
    /// directory (or a parent of it). Syncing such paths would be dangerous.
    ///
    /// # Errors
    ///
    /// Returns `InvalidSyncDir` if the path is the filesystem root, the user's
    /// home directory, or a parent of it.
    pub fn validate_sync_dir(sync_dir: &Path) -> Result<()> {
        use crate::error::Error;

        // Reject filesystem root
        if sync_dir == Path::new("/") {
            return Err(Error::InvalidSyncDir(
                "cannot synchronize the system root (/)".to_string(),
            ));
        }

        // Reject the home directory or any of its ancestors
        if let Some(home) = dirs::home_dir() {
            let sync_with_sep = format!("{}/", sync_dir.display());
            let home_with_sep = format!("{}/", home.display());
            if home_with_sep.starts_with(&sync_with_sep) {
                return Err(Error::InvalidSyncDir(
                    "cannot synchronize the home directory or one of its parents".to_string(),
                ));
            }
        }

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

    #[must_use]
    pub fn rag_dir(&self) -> PathBuf {
        self.data_dir.join("rag")
    }

    #[must_use]
    pub fn syncignore_path(&self) -> PathBuf {
        self.data_dir.join("syncignore")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn test_config() -> Config {
        Config {
            instance_url: "https://test.mycozy.cloud".to_string(),
            sync_dir: PathBuf::from("/tmp/sync"),
            data_dir: PathBuf::from("/tmp/data"),
            oauth_client: Some(OAuthClient {
                instance_url: "https://test.mycozy.cloud".to_string(),
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
                registration_access_token: "reg-token".to_string(),
                access_token: Some("access".to_string()),
                refresh_token: Some("refresh".to_string()),
            }),
            last_seq: None,
            api_key: None,
        }
    }

    #[test]
    fn save_sets_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let config = test_config();
        config.save(&path).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn api_key_round_trips_through_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = test_config();
        config.api_key = Some("sk-test-key".to_string());
        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap().unwrap();
        assert_eq!(loaded.api_key, Some("sk-test-key".to_string()));
    }

    #[test]
    fn old_config_without_api_key_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // Write a JSON object that has no api_key field (simulates existing config)
        std::fs::write(
            &path,
            r#"{"instance_url":"https://x.mycozy.cloud","sync_dir":"/tmp/sync","data_dir":"/tmp/data","oauth_client":null,"last_seq":null}"#,
        ).unwrap();
        let loaded = Config::load(&path).unwrap().unwrap();
        assert_eq!(loaded.api_key, None);
    }

    #[test]
    fn validate_sync_dir_rejects_root() {
        let result = Config::validate_sync_dir(Path::new("/"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("system root"), "unexpected error: {msg}");
    }

    #[test]
    fn validate_sync_dir_rejects_home() {
        let home = dirs::home_dir().unwrap();
        let result = Config::validate_sync_dir(&home);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("home directory"), "unexpected error: {msg}");
    }

    #[test]
    fn validate_sync_dir_rejects_parent_of_home() {
        let home = dirs::home_dir().unwrap();
        if let Some(parent) = home.parent() {
            if parent != Path::new("/") {
                let result = Config::validate_sync_dir(parent);
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn validate_sync_dir_accepts_subdir_of_home() {
        let home = dirs::home_dir().unwrap();
        let sync_dir = home.join("Cozy Drive");
        let result = Config::validate_sync_dir(&sync_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_sync_dir_accepts_path_outside_home() {
        let result = Config::validate_sync_dir(Path::new("/data/cozy"));
        assert!(result.is_ok());
    }

    #[test]
    fn save_load_roundtrip_preserves_oauth_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let config = test_config();
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap().unwrap();
        let oauth = loaded.oauth_client.unwrap();
        assert_eq!(oauth.client_secret, "secret");
        assert_eq!(oauth.registration_access_token, "reg-token");
        assert_eq!(oauth.access_token, Some("access".to_string()));
        assert_eq!(oauth.refresh_token, Some("refresh".to_string()));
    }
}
