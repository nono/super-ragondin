use cozy_desktop::config::Config;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn load_returns_none_when_file_does_not_exist() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("nonexistent.json");

    let result = Config::load(&config_path).unwrap();

    assert!(result.is_none());
}

#[test]
fn save_and_load_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");
    let sync_dir = temp_dir.path().join("sync");
    let data_dir = temp_dir.path().join("data");

    let config = Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: sync_dir.clone(),
        data_dir: data_dir.clone(),
        oauth_client: None,
        last_seq: None,
    };

    config.save(&config_path).unwrap();

    let loaded = Config::load(&config_path).unwrap().unwrap();

    assert_eq!(loaded.instance_url, "https://test.mycozy.cloud");
    assert_eq!(loaded.sync_dir, sync_dir);
    assert_eq!(loaded.data_dir, data_dir);
    assert!(loaded.oauth_client.is_none());
    assert!(loaded.last_seq.is_none());
}

#[test]
fn save_creates_parent_directories() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir
        .path()
        .join("nested")
        .join("dir")
        .join("config.json");

    let config = Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: PathBuf::from("/tmp/sync"),
        data_dir: PathBuf::from("/tmp/data"),
        oauth_client: None,
        last_seq: None,
    };

    config.save(&config_path).unwrap();

    assert!(config_path.exists());
}

#[test]
fn staging_dir_returns_correct_path() {
    let config = Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: PathBuf::from("/home/user/Cozy"),
        data_dir: PathBuf::from("/home/user/.local/share/cozy-desktop"),
        oauth_client: None,
        last_seq: None,
    };

    let staging = config.staging_dir();

    assert_eq!(
        staging,
        PathBuf::from("/home/user/.local/share/cozy-desktop/staging")
    );
}

#[test]
fn store_dir_returns_correct_path() {
    let config = Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: PathBuf::from("/home/user/Cozy"),
        data_dir: PathBuf::from("/home/user/.local/share/cozy-desktop"),
        oauth_client: None,
        last_seq: None,
    };

    let store = config.store_dir();

    assert_eq!(
        store,
        PathBuf::from("/home/user/.local/share/cozy-desktop/store")
    );
}

#[test]
fn save_and_load_with_last_seq() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");

    let config = Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: temp_dir.path().join("sync"),
        data_dir: temp_dir.path().join("data"),
        oauth_client: None,
        last_seq: Some("42-abc123".to_string()),
    };

    config.save(&config_path).unwrap();
    let loaded = Config::load(&config_path).unwrap().unwrap();

    assert_eq!(loaded.last_seq, Some("42-abc123".to_string()));
}
