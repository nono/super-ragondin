use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use super_ragondin_sync::config::Config;
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::remote::auth::OAuthClient;
use super_ragondin_sync::remote::client::CozyClient;
use super_ragondin_sync::store::TreeStore;
use super_ragondin_sync::sync::engine::SyncEngine;
use tauri_specta::Event;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub static TRAY_IDLE_BYTES: &[u8] = include_bytes!("../icons/tray-idle.png");
pub static TRAY_SYNCING_BYTES: &[u8] = include_bytes!("../icons/tray-syncing.png");

/// Emitted when OAuth completes successfully.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AuthCompleteEvent;

/// Emitted when OAuth fails.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AuthErrorEvent {
    pub message: String,
}

/// Extract `(code, state)` from an HTTP GET request line.
///
/// Input: `"GET /callback?code=X&state=Y HTTP/1.1"`
pub fn parse_callback(request_line: &str) -> Option<(String, String)> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        match key {
            "code" => {
                code = Some(
                    urlencoding::decode(parts.next().unwrap_or(""))
                        .unwrap_or_default()
                        .into_owned(),
                );
            }
            "state" => {
                state = Some(
                    urlencoding::decode(parts.next().unwrap_or(""))
                        .unwrap_or_default()
                        .into_owned(),
                );
            }
            _ => {}
        }
    }
    let code = code?;
    if code.is_empty() {
        return None;
    }
    Some((code, state?))
}

async fn run_auth_flow(
    app: &tauri::AppHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::load(&config_path())?.ok_or("No config — run init first")?;

    let mut oauth =
        OAuthClient::register(&config.instance_url, "Super Ragondin", "super-ragondin").await?;

    let state = uuid::Uuid::new_v4().to_string();
    let auth_url = oauth.authorization_url(&state);

    let Ok(listener) = TcpListener::bind("127.0.0.1:8080").await else {
        AuthErrorEvent {
            message: "Port 8080 is already in use. Close other applications and try again."
                .to_string(),
        }
        .emit(app)?;
        return Ok(());
    };

    if let Err(e) = opener::open(&auth_url) {
        AuthErrorEvent {
            message: format!("Failed to open browser: {e}"),
        }
        .emit(app)?;
        return Ok(());
    }

    let (mut stream, _) =
        match tokio::time::timeout(Duration::from_secs(300), listener.accept()).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                AuthErrorEvent {
                    message: format!("Failed to accept connection: {e}"),
                }
                .emit(app)?;
                return Ok(());
            }
            Err(_) => {
                AuthErrorEvent {
                    message: "OAuth callback timed out after 5 minutes.".to_string(),
                }
                .emit(app)?;
                return Ok(());
            }
        };
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    let Some((code, received_state)) = parse_callback(request_line.trim()) else {
        AuthErrorEvent {
            message: "Could not parse OAuth callback.".to_string(),
        }
        .emit(app)?;
        return Ok(());
    };

    if received_state != state {
        AuthErrorEvent {
            message: "OAuth state mismatch — possible CSRF attempt.".to_string(),
        }
        .emit(app)?;
        return Ok(());
    }

    let response = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/html\r\n\r\n",
        "<html><body>Authorization complete. You can close this tab.</body></html>"
    );
    stream.write_all(response.as_bytes()).await?;
    drop(stream);
    drop(listener);

    oauth.exchange_code(&code).await?;

    let mut updated = config;
    updated.oauth_client = Some(oauth);
    updated.save(&config_path())?;

    AuthCompleteEvent.emit(app)?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn start_auth(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_auth_flow(&app).await {
            let _ = AuthErrorEvent {
                message: e.to_string(),
            }
            .emit(&app);
        }
    });
}

/// Application state returned by `get_app_state`.
///
/// **Frontend contract:** serializes as `"Unconfigured"` / `"Unauthenticated"` / `"Ready"` —
/// these string values are matched verbatim in `App.svelte`.
#[derive(Debug, serde::Serialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "PascalCase")]
pub enum AppState {
    Unconfigured,
    Unauthenticated,
    Ready,
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin")
        .join("config.json")
}

pub fn app_state_from_config(config: Option<&Config>) -> AppState {
    config.map_or(AppState::Unconfigured, |c| {
        if c.oauth_client
            .as_ref()
            .and_then(|o| o.access_token.as_ref())
            .is_some()
        {
            AppState::Ready
        } else {
            AppState::Unauthenticated
        }
    })
}

pub fn init_config_to(
    instance_url: String,
    sync_dir: String,
    config_path: &std::path::Path,
) -> Result<(), String> {
    let sync_dir = PathBuf::from(sync_dir);
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin");

    let config = Config {
        instance_url,
        sync_dir: sync_dir.clone(),
        data_dir: data_dir.clone(),
        oauth_client: None,
        last_seq: None,
    };
    fs::create_dir_all(&sync_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(config.staging_dir()).map_err(|e| e.to_string())?;
    config.save(config_path).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn init_config(instance_url: String, sync_dir: String) -> Result<(), String> {
    init_config_to(instance_url, sync_dir, &config_path())
}

#[tauri::command]
#[specta::specta]
pub fn get_app_state() -> AppState {
    let config = Config::load(&config_path()).ok().flatten();
    app_state_from_config(config.as_ref())
}

/// Idempotency guard to prevent starting the sync loop twice.
#[derive(Default)]
pub struct SyncGuard(pub Mutex<bool>);

/// Build the tauri-specta builder — shared by `main()` and the export test.
pub fn make_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            get_app_state,
            init_config,
            start_auth,
            start_sync,
        ])
        .events(tauri_specta::collect_events![
            AuthCompleteEvent,
            AuthErrorEvent,
            SyncStatusEvent,
        ])
}

/// Sync state reported via `sync_status` events.
///
/// **Frontend contract:** serializes as `"Syncing"` / `"Idle"` — matches
/// `Syncing.svelte` which checks `event.payload.status`.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "PascalCase")]
pub enum SyncState {
    Syncing,
    Idle,
}

/// Emitted each sync loop iteration.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct SyncStatusEvent {
    pub status: SyncState,
    pub last_sync: Option<String>,
}

/// Run one sync cycle, returning an ISO 8601 timestamp on success.
///
/// # Errors
///
/// Returns an error if config loading, store opening, or the sync cycle fails.
pub async fn do_sync_cycle() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = config_path();
    let mut config = Config::load(&path)?.ok_or("No config — run init first")?;
    let access_token = config
        .oauth_client
        .as_ref()
        .ok_or("No OAuth client")?
        .access_token
        .as_ref()
        .ok_or("No access token")?
        .clone();

    let client = CozyClient::new(&config.instance_url, &access_token);

    let store = TreeStore::open(&config.store_dir())?;
    let rules = IgnoreRules::load(Some(&config.syncignore_path()));
    let mut engine = SyncEngine::new(store, config.sync_dir.clone(), config.staging_dir(), rules);

    let since = config.last_seq.as_deref();
    let new_seq = engine
        .fetch_and_apply_remote_changes(&client, since)
        .await?;

    engine.run_cycle_async(&client).await?;

    // Persist sequence number after the sync cycle completes to avoid advancing
    // past a failed cycle (which would silently drop remote changes).
    config.last_seq = Some(new_seq);
    config.save(&path)?;

    Ok(chrono::Utc::now().to_rfc3339())
}

/// Run the sync loop indefinitely: emit `sync_status` events and sleep 30s between cycles.
///
/// This runs on a dedicated OS thread with its own tokio runtime to work around
/// HRTB lifetime constraints in `SyncEngine::run_cycle_async`.
pub fn run_sync_loop(app: &tauri::AppHandle) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("Failed to build sync runtime: {e}");
            return;
        }
    };
    rt.block_on(async {
        let update_tray_icon = |state: &SyncState| {
            let bytes = match state {
                SyncState::Syncing => TRAY_SYNCING_BYTES,
                SyncState::Idle => TRAY_IDLE_BYTES,
            };
            if let Some(tray) = app.tray_by_id("main-tray") {
                if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                    tray.set_icon(Some(icon)).ok();
                }
            }
        };

        let mut last_sync: Option<String> = None;
        loop {
            update_tray_icon(&SyncState::Syncing);
            let _ = SyncStatusEvent {
                status: SyncState::Syncing,
                last_sync: last_sync.clone(),
            }
            .emit(app);

            last_sync = match do_sync_cycle().await {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::error!("Sync cycle failed: {e}");
                    last_sync
                }
            };

            update_tray_icon(&SyncState::Idle);
            let _ = SyncStatusEvent {
                status: SyncState::Idle,
                last_sync: last_sync.clone(),
            }
            .emit(app);

            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    tracing::info!("Sync loop exited");
}

/// Start the background sync loop (idempotent: does nothing if already running).
#[tauri::command]
#[specta::specta]
pub async fn start_sync(
    app: tauri::AppHandle,
    guard: tauri::State<'_, SyncGuard>,
) -> Result<(), String> {
    let mut running = guard.0.lock().map_err(|e| e.to_string())?;
    if *running {
        return Ok(());
    }
    *running = true;
    drop(running);

    std::thread::spawn(move || run_sync_loop(&app));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_extracts_code_and_state() {
        let line = "GET /callback?code=abc123&state=xyz HTTP/1.1";
        let result = parse_callback(line);
        assert_eq!(result, Some(("abc123".to_string(), "xyz".to_string())));
    }

    #[test]
    fn parse_callback_missing_code_returns_none() {
        let line = "GET /callback?state=xyz HTTP/1.1";
        assert_eq!(parse_callback(line), None);
    }

    #[test]
    fn parse_callback_missing_state_returns_none() {
        let line = "GET /callback?code=abc HTTP/1.1";
        assert_eq!(parse_callback(line), None);
    }

    #[test]
    fn parse_callback_invalid_format_returns_none() {
        assert_eq!(parse_callback("not a valid request"), None);
    }

    #[test]
    fn parse_callback_reversed_order_works() {
        let line = "GET /callback?state=Y&code=X HTTP/1.1";
        let result = parse_callback(line);
        assert_eq!(result, Some(("X".to_string(), "Y".to_string())));
    }

    #[test]
    fn parse_callback_extra_params_works() {
        let line = "GET /callback?code=X&scope=foo&state=Y HTTP/1.1";
        let result = parse_callback(line);
        assert_eq!(result, Some(("X".to_string(), "Y".to_string())));
    }

    #[test]
    fn parse_callback_empty_code_returns_none() {
        let line = "GET /callback?code=&state=Y HTTP/1.1";
        assert_eq!(parse_callback(line), None);
    }

    fn config_no_oauth() -> Config {
        Config {
            instance_url: "https://test.mycozy.cloud".to_string(),
            sync_dir: PathBuf::from("/tmp/sync"),
            data_dir: PathBuf::from("/tmp/data"),
            oauth_client: None,
            last_seq: None,
        }
    }

    fn oauth_no_token() -> OAuthClient {
        OAuthClient {
            instance_url: "https://test.mycozy.cloud".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            registration_access_token: "reg".to_string(),
            access_token: None,
            refresh_token: None,
        }
    }

    fn oauth_with_token() -> OAuthClient {
        OAuthClient {
            access_token: Some("tok".to_string()),
            ..oauth_no_token()
        }
    }

    #[test]
    fn init_config_creates_dirs_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let sync_dir = dir.path().join("sync");

        let instance_url = "https://alice.mycozy.cloud".to_string();
        let result = init_config_to(
            instance_url.clone(),
            sync_dir.to_str().unwrap().to_string(),
            &dir.path().join("config.json"),
        );

        assert!(result.is_ok(), "init_config_to should succeed: {result:?}");
        assert!(sync_dir.exists(), "sync_dir should be created");
        let loaded = Config::load(&dir.path().join("config.json"))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.instance_url, instance_url);
        assert!(loaded.oauth_client.is_none());
    }

    #[test]
    fn no_config_is_unconfigured() {
        assert_eq!(app_state_from_config(None), AppState::Unconfigured);
    }

    #[test]
    fn config_without_oauth_is_unauthenticated() {
        let c = config_no_oauth();
        assert_eq!(app_state_from_config(Some(&c)), AppState::Unauthenticated);
    }

    #[test]
    fn config_with_oauth_but_no_token_is_unauthenticated() {
        let mut c = config_no_oauth();
        c.oauth_client = Some(oauth_no_token());
        assert_eq!(app_state_from_config(Some(&c)), AppState::Unauthenticated);
    }

    #[test]
    fn config_with_token_is_ready() {
        let mut c = config_no_oauth();
        c.oauth_client = Some(oauth_with_token());
        assert_eq!(app_state_from_config(Some(&c)), AppState::Ready);
    }

    #[test]
    fn sync_guard_starts_unlocked() {
        let guard = SyncGuard::default();
        assert!(!*guard.0.lock().unwrap(), "guard should start false");
    }

    #[test]
    fn sync_guard_can_be_set() {
        let guard = SyncGuard::default();
        *guard.0.lock().unwrap() = true;
        assert!(*guard.0.lock().unwrap());
    }

    #[test]
    fn tray_icon_bytes_are_valid_images() {
        use tauri::image::Image;
        assert!(
            Image::from_bytes(TRAY_IDLE_BYTES).is_ok(),
            "tray-idle.png must be a valid image"
        );
        assert!(
            Image::from_bytes(TRAY_SYNCING_BYTES).is_ok(),
            "tray-syncing.png must be a valid image"
        );
    }

    #[test]
    #[ignore]
    fn export_bindings() {
        make_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../../gui-frontend/src/bindings.ts",
            )
            .expect("Failed to export bindings");
    }
}
