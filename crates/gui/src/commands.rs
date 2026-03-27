use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use super_ragondin_sync::config::Config;
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::local::watcher::WatchEventKind;
use super_ragondin_sync::remote::auth::OAuthClient;
use super_ragondin_sync::remote::client::CozyClient;
use super_ragondin_sync::store::TreeStore;
use super_ragondin_sync::sync::engine::SyncEngine;
use super_ragondin_sync::watcher_mux::{start_watchers, SyncTrigger};
use tauri_specta::Event;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[cfg(feature = "tray")]
pub static TRAY_ID: &str = "main-tray";
#[cfg(feature = "tray")]
pub static TRAY_IDLE_BYTES: &[u8] = include_bytes!("../icons/tray-idle.png");
#[cfg(feature = "tray")]
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
    api_key: Option<String>,
    config_path: &std::path::Path,
) -> Result<(), String> {
    let sync_dir = PathBuf::from(sync_dir);

    Config::validate_sync_dir(&sync_dir).map_err(|e| e.to_string())?;

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin");

    // Preserve OAuth credentials and sync position if the instance URL hasn't changed.
    let existing = Config::load(config_path).map_err(|e| e.to_string())?;
    let (oauth_client, last_seq) = existing
        .filter(|c| c.instance_url == instance_url)
        .map_or((None, None), |c| (c.oauth_client, c.last_seq));

    let config = Config {
        instance_url,
        sync_dir: sync_dir.clone(),
        data_dir: data_dir.clone(),
        oauth_client,
        last_seq,
        api_key,
    };
    fs::create_dir_all(&sync_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(config.staging_dir()).map_err(|e| e.to_string())?;
    config.save(config_path).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn init_config(
    instance_url: String,
    sync_dir: String,
    api_key: Option<String>,
) -> Result<(), String> {
    init_config_to(instance_url, sync_dir, api_key, &config_path())
}

/// Testable core of `set_api_key`: loads the config at `config_path`,
/// updates only the `api_key` field (clearing it when `api_key` is empty),
/// and saves it back.
pub fn set_api_key_to(api_key: String, config_path: &std::path::Path) -> Result<(), String> {
    let mut config = Config::load(config_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;
    config.api_key = if api_key.is_empty() {
        None
    } else {
        Some(api_key)
    };
    config.save(config_path).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn set_api_key(api_key: String) -> Result<(), String> {
    set_api_key_to(api_key, &config_path())
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

/// Testable core of `get_recent_files`: opens the store, stats each file, returns top 10.
pub fn get_recent_files_from(
    store_dir: &std::path::Path,
    sync_dir: &std::path::Path,
) -> Result<Vec<String>, String> {
    use super_ragondin_sync::model::NodeType;

    let store = TreeStore::open(store_dir).map_err(|e| e.to_string())?;
    let synced = store.list_all_synced().map_err(|e| e.to_string())?;

    let mut entries: Vec<(std::time::SystemTime, String)> = synced
        .into_iter()
        .filter(|r| r.node_type == NodeType::File)
        .filter_map(|r| {
            // Validate path doesn't escape sync_dir (path traversal protection)
            let abs = sync_dir.join(&r.rel_path);
            let canonical_sync = sync_dir.canonicalize().ok()?;
            let canonical_abs = abs.canonicalize().ok()?;
            if !canonical_abs.starts_with(&canonical_sync) {
                return None;
            }
            // Silently skip files that no longer exist on disk
            let mtime = std::fs::metadata(&abs).ok()?.modified().ok()?;
            Some((mtime, r.rel_path))
        })
        .collect();

    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().take(10).map(|(_, p)| p).collect())
}

#[tauri::command]
#[specta::specta]
pub fn get_recent_files() -> Result<Vec<String>, String> {
    let config = Config::load(&config_path())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;
    get_recent_files_from(&config.store_dir(), &config.sync_dir)
}

/// Testable core: loads config from `config_path`, runs `CodeModeEngine`.
pub async fn ask_question_from(
    question: &str,
    config_path: &std::path::Path,
) -> Result<String, String> {
    use super_ragondin_codemode::engine::CodeModeEngine;
    use super_ragondin_rag::config::RagConfig;

    tracing::info!(question, "✦ Ask: starting");

    let config = Config::load(config_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;

    let api_key = config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "NoApiKey".to_string())?
        .to_string();

    let mut rag_config = RagConfig::from_env_with_db_path(config.rag_dir());
    rag_config.api_key = api_key;

    let engine = CodeModeEngine::new(rag_config, config.sync_dir, None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "✦ Ask: failed to create engine");
            e.to_string()
        })?;

    engine.ask(question, None).await.map_err(|e| {
        tracing::error!(error = %e, "✦ Ask: query failed");
        e.to_string()
    })
}

#[tauri::command]
#[specta::specta]
pub async fn ask_question(question: String) -> Result<String, String> {
    ask_question_from(&question, &config_path()).await
}

/// Testable core: loads config from `config_path`, runs `SuggestionEngine`.
pub async fn get_suggestions_from(config_path: &std::path::Path) -> Result<Vec<String>, String> {
    use super_ragondin_codemode::suggestions::{NoFilesIndexed, SuggestionEngine};
    use super_ragondin_rag::config::RagConfig;

    let config = Config::load(config_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;

    let api_key = config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "NoApiKey".to_string())?
        .to_string();

    let mut rag_config = RagConfig::from_env_with_db_path(config.rag_dir());
    rag_config.api_key = api_key;

    let engine = SuggestionEngine::new(rag_config, config.sync_dir)
        .await
        .map_err(|e| e.to_string())?;

    engine.generate(None).await.map_err(|e| {
        if e.downcast_ref::<NoFilesIndexed>().is_some() {
            "NoFilesIndexed".to_string()
        } else {
            e.to_string()
        }
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_suggestions() -> Result<Vec<String>, String> {
    get_suggestions_from(&config_path()).await
}

/// Build the tauri-specta builder — shared by `main()` and the export test.
pub fn make_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            get_app_state,
            init_config,
            set_api_key,
            start_auth,
            start_sync,
            get_recent_files,
            get_suggestions,
            ask_question,
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

    // Persist sequence number immediately after remote changes are applied so
    // that a failure in run_cycle_async does not cause the same remote changes
    // to be re-fetched and re-applied on the next cycle.
    config.last_seq = Some(new_seq);
    config.save(&path)?;

    engine.run_cycle_async(&client).await?;

    let api_key = super_ragondin_rag::config::RagConfig::resolve_api_key(config.api_key.as_deref());
    let synced = engine.store().list_all_synced()?;
    super_ragondin_rag::indexer::reconcile_if_configured(
        api_key,
        config.rag_dir(),
        &synced,
        &config.sync_dir,
    )
    .await;

    Ok(chrono::Utc::now().to_rfc3339())
}

/// Run the sync loop indefinitely: emit `sync_status` events and react to
/// local filesystem and remote WebSocket events, with a 30s fallback.
///
/// This runs on a dedicated OS thread with its own tokio runtime to work around
/// HRTB lifetime constraints in `SyncEngine::run_cycle_async`.
#[allow(clippy::too_many_lines)]
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

    let config = match Config::load(&config_path()) {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::error!("No config found, cannot start sync loop");
            return;
        }
        Err(e) => {
            tracing::error!("Failed to load config: {e}");
            return;
        }
    };

    let (rx, cancel) = match start_watchers(&config) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!("Failed to initialize filesystem watcher: {e}");
            return;
        }
    };

    rt.block_on(async {
        #[cfg(feature = "tray")]
        let update_tray_icon = |state: &SyncState| {
            let bytes = match state {
                SyncState::Syncing => TRAY_SYNCING_BYTES,
                SyncState::Idle => TRAY_IDLE_BYTES,
            };
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                    tray.set_icon(Some(icon)).ok();
                }
            }
        };

        let mut last_sync_ts: Option<String> = None;
        let mut last_sync: Option<Instant> = None;
        let mut pending_writes: HashMap<PathBuf, Instant> = HashMap::new();
        let mut pending = true;
        let debounce = Duration::from_millis(200);
        let write_timeout = Duration::from_secs(30);

        loop {
            let debounce_ok = last_sync.is_none_or(|t| t.elapsed() > debounce);

            // Expire stale pending writes (safety net for mmap / non-close writers)
            let now = Instant::now();
            let expired: Vec<_> = pending_writes
                .iter()
                .filter(|&(_, &t)| now.duration_since(t) > write_timeout)
                .map(|(p, _)| p.clone())
                .collect();
            for path in &expired {
                tracing::debug!(path = %path.display(), "⏰ Pending write timed out, releasing");
                pending_writes.remove(path);
            }
            if !expired.is_empty() {
                pending = true;
            }

            if pending && debounce_ok && pending_writes.is_empty() {
                pending = false;

                #[cfg(feature = "tray")]
                update_tray_icon(&SyncState::Syncing);
                let _ = SyncStatusEvent {
                    status: SyncState::Syncing,
                    last_sync: last_sync_ts.clone(),
                }
                .emit(app);

                last_sync_ts = match do_sync_cycle().await {
                    Ok(ts) => Some(ts),
                    Err(e) => {
                        tracing::error!("Sync cycle failed: {e}");
                        last_sync_ts
                    }
                };

                #[cfg(feature = "tray")]
                update_tray_icon(&SyncState::Idle);
                let _ = SyncStatusEvent {
                    status: SyncState::Idle,
                    last_sync: last_sync_ts.clone(),
                }
                .emit(app);

                last_sync = Some(Instant::now());
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(trigger) => {
                    match &trigger {
                        SyncTrigger::Local(event) => {
                            tracing::debug!(event = ?event, "👁️ Local watch event received");
                            match event.kind {
                                WatchEventKind::Create | WatchEventKind::Modify
                                    if !event.is_dir =>
                                {
                                    pending_writes
                                        .entry(event.path.clone())
                                        .or_insert_with(Instant::now);
                                }
                                WatchEventKind::CloseWrite => {
                                    if pending_writes.remove(&event.path).is_some() {
                                        tracing::debug!(
                                            path = %event.path.display(),
                                            "✅ Write complete (CLOSE_WRITE)"
                                        );
                                    }
                                }
                                WatchEventKind::Delete => {
                                    pending_writes.remove(&event.path);
                                }
                                _ => {}
                            }
                        }
                        SyncTrigger::Remote => {
                            tracing::info!("🔌 Remote change notification received");
                        }
                    }
                    pending = true;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if last_sync.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                        pending = true;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    tracing::error!("❌ All event sources disconnected");
                    break;
                }
            }
        }
    });
    cancel.cancel();
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
            api_key: None,
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

    fn saved_config_with_oauth(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let sync_dir = dir.path().join("sync");
        let config_path = dir.path().join("config.json");
        Config {
            instance_url: "https://alice.mycozy.cloud".to_string(),
            sync_dir: sync_dir.clone(),
            data_dir: dir.path().to_path_buf(),
            oauth_client: Some(oauth_with_token()),
            last_seq: Some("42-abc".to_string()),
            api_key: None,
        }
        .save(&config_path)
        .unwrap();
        (sync_dir, config_path)
    }

    #[test]
    fn init_config_creates_dirs_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let sync_dir = dir.path().join("sync");

        let instance_url = "https://alice.mycozy.cloud".to_string();
        let result = init_config_to(
            instance_url.clone(),
            sync_dir.to_str().unwrap().to_string(),
            None,
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
    fn init_config_to_preserves_oauth_when_instance_url_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (sync_dir, config_path) = saved_config_with_oauth(&dir);

        let result = init_config_to(
            "https://alice.mycozy.cloud".to_string(),
            sync_dir.to_str().unwrap().to_string(),
            Some("sk-new-key".to_string()),
            &config_path,
        );
        assert!(result.is_ok());

        let loaded = Config::load(&config_path).unwrap().unwrap();
        assert_eq!(loaded.api_key, Some("sk-new-key".to_string()));
        assert!(loaded.oauth_client.is_some());
        assert_eq!(loaded.last_seq, Some("42-abc".to_string()));
    }

    #[test]
    fn init_config_to_clears_oauth_when_instance_url_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (sync_dir, config_path) = saved_config_with_oauth(&dir);

        let result = init_config_to(
            "https://bob.mycozy.cloud".to_string(),
            sync_dir.to_str().unwrap().to_string(),
            None,
            &config_path,
        );
        assert!(result.is_ok());

        let loaded = Config::load(&config_path).unwrap().unwrap();
        assert!(loaded.oauth_client.is_none());
        assert!(loaded.last_seq.is_none());
    }

    #[test]
    fn init_config_to_saves_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let sync_dir = dir.path().join("sync");
        let config_path = dir.path().join("config.json");
        let result = init_config_to(
            "https://alice.mycozy.cloud".to_string(),
            sync_dir.to_str().unwrap().to_string(),
            Some("sk-openrouter-test".to_string()),
            &config_path,
        );
        assert!(result.is_ok());
        let loaded = super_ragondin_sync::config::Config::load(&config_path)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.api_key, Some("sk-openrouter-test".to_string()));
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
    #[cfg(feature = "tray")]
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
    fn get_recent_files_returns_top_files_by_mtime() {
        use super_ragondin_sync::model::{LocalFileId, NodeType, RemoteId, SyncedRecord};

        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        let sync_dir = dir.path().join("sync");
        std::fs::create_dir_all(&sync_dir).unwrap();

        // Write two files to disk with a small mtime gap
        let file_a = sync_dir.join("a.txt");
        let file_b = sync_dir.join("b.txt");
        std::fs::write(&file_a, "a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file_b, "b").unwrap();

        // Insert matching SyncedRecords into the store
        let store = TreeStore::open(&store_dir).unwrap();
        let record_a = SyncedRecord {
            local_id: LocalFileId::new(1, 1),
            remote_id: RemoteId::new("ra"),
            rel_path: "a.txt".to_string(),
            md5sum: None,
            size: None,
            rev: "1-a".to_string(),
            node_type: NodeType::File,
            local_name: None,
            local_parent_id: None,
            remote_name: None,
            remote_parent_id: None,
        };
        let record_b = SyncedRecord {
            local_id: LocalFileId::new(1, 2),
            remote_id: RemoteId::new("rb"),
            rel_path: "b.txt".to_string(),
            md5sum: None,
            size: None,
            rev: "1-b".to_string(),
            node_type: NodeType::File,
            local_name: None,
            local_parent_id: None,
            remote_name: None,
            remote_parent_id: None,
        };
        store.insert_synced(&record_a).unwrap();
        store.insert_synced(&record_b).unwrap();

        let result = get_recent_files_from(&store_dir, &sync_dir);
        assert!(result.is_ok());
        let files = result.unwrap();
        assert_eq!(files.len(), 2);
        // b.txt was written later so it must appear first
        assert_eq!(files[0], "b.txt");
        assert_eq!(files[1], "a.txt");
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

    #[tokio::test]
    async fn ask_question_no_api_key_returns_error() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.json");
        let config = super_ragondin_sync::config::Config {
            instance_url: "https://x.mycozy.cloud".to_string(),
            sync_dir: dir.path().join("sync"),
            data_dir: dir.path().to_path_buf(),
            oauth_client: None,
            last_seq: None,
            api_key: None,
        };
        config.save(&config_path)?;

        let result = ask_question_from("What is in my files?", &config_path).await;
        assert_eq!(result, Err("NoApiKey".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn get_suggestions_no_api_key_returns_error() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.json");
        // Save a config with no api_key
        let config = super_ragondin_sync::config::Config {
            instance_url: "https://x.mycozy.cloud".to_string(),
            sync_dir: dir.path().join("sync"),
            data_dir: dir.path().to_path_buf(),
            oauth_client: None,
            last_seq: None,
            api_key: None,
        };
        config.save(&config_path)?;

        let result = get_suggestions_from(&config_path).await;
        assert_eq!(result, Err("NoApiKey".to_string()));
        Ok(())
    }

    #[test]
    fn set_api_key_to_updates_only_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let (_sync_dir, config_path) = saved_config_with_oauth(&dir);

        let result = set_api_key_to("sk-new".to_string(), &config_path);
        assert!(result.is_ok());

        let loaded = Config::load(&config_path).unwrap().unwrap();
        assert_eq!(loaded.api_key, Some("sk-new".to_string()));
        // OAuth and other fields must be preserved
        assert!(loaded.oauth_client.is_some());
        assert_eq!(loaded.instance_url, "https://alice.mycozy.cloud");
        assert_eq!(loaded.last_seq, Some("42-abc".to_string()));
    }

    #[test]
    fn set_api_key_to_empty_string_clears_key() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        Config {
            instance_url: "https://alice.mycozy.cloud".to_string(),
            sync_dir: dir.path().join("sync"),
            data_dir: dir.path().to_path_buf(),
            oauth_client: None,
            last_seq: None,
            api_key: Some("sk-old".to_string()),
        }
        .save(&config_path)
        .unwrap();

        let result = set_api_key_to(String::new(), &config_path);
        assert!(result.is_ok());

        let loaded = Config::load(&config_path).unwrap().unwrap();
        assert_eq!(loaded.api_key, None);
    }

    #[test]
    fn set_api_key_to_no_config_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        let result = set_api_key_to("sk-x".to_string(), &config_path);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_suggestions_no_files_indexed_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config = super_ragondin_sync::config::Config {
            instance_url: "https://x.mycozy.cloud".to_string(),
            sync_dir: dir.path().join("sync"),
            data_dir: dir.path().to_path_buf(),
            oauth_client: None,
            last_seq: None,
            api_key: Some("sk-test".to_string()),
        };
        std::fs::create_dir_all(config.rag_dir()).unwrap();
        config.save(&config_path).unwrap();

        let result = get_suggestions_from(&config_path).await;
        assert_eq!(result, Err("NoFilesIndexed".to_string()));
    }
}
