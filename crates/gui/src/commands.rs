use std::fs;
use std::path::PathBuf;
use super_ragondin_sync::config::Config;
use super_ragondin_sync::remote::auth::OAuthClient;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(serde::Serialize, Clone)]
pub struct AuthError {
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
        let val = parts.next().unwrap_or("").to_string();
        match key {
            "code" => code = Some(val),
            "state" => state = Some(val),
            _ => {}
        }
    }
    Some((code?, state?))
}

async fn run_auth_flow(
    app: &tauri::AppHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::load(&config_path())?.ok_or("No config — run init first")?;

    let mut oauth =
        OAuthClient::register(&config.instance_url, "Super Ragondin", "super-ragondin").await?;

    let state = uuid::Uuid::new_v4().to_string();
    let auth_url = oauth.authorization_url(&state);

    opener::open(&auth_url)?;

    let listener = match TcpListener::bind("127.0.0.1:8080").await {
        Ok(l) => l,
        Err(_) => {
            app.emit(
                "auth_error",
                AuthError {
                    message: "Port 8080 is already in use. Close other applications and try again."
                        .to_string(),
                },
            )?;
            return Ok(());
        }
    };

    let (mut stream, _) = listener.accept().await?;
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    let (code, received_state) = match parse_callback(request_line.trim()) {
        Some(pair) => pair,
        None => {
            app.emit(
                "auth_error",
                AuthError {
                    message: "Could not parse OAuth callback.".to_string(),
                },
            )?;
            return Ok(());
        }
    };

    if received_state != state {
        app.emit(
            "auth_error",
            AuthError {
                message: "OAuth state mismatch — possible CSRF attempt.".to_string(),
            },
        )?;
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

    app.emit("auth_complete", ())?;
    Ok(())
}

#[tauri::command]
pub fn start_auth(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_auth_flow(&app).await {
            let _ = app.emit(
                "auth_error",
                AuthError {
                    message: e.to_string(),
                },
            );
        }
    });
}

#[derive(Debug, serde::Serialize, PartialEq)]
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
    match config {
        None => AppState::Unconfigured,
        Some(c) => {
            if c.oauth_client
                .as_ref()
                .and_then(|o| o.access_token.as_ref())
                .is_some()
            {
                AppState::Ready
            } else {
                AppState::Unauthenticated
            }
        }
    }
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

    fs::create_dir_all(&sync_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(data_dir.join("staging")).map_err(|e| e.to_string())?;

    let config = Config {
        instance_url,
        sync_dir,
        data_dir,
        oauth_client: None,
        last_seq: None,
    };
    config.save(config_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn init_config(instance_url: String, sync_dir: String) -> Result<(), String> {
    init_config_to(instance_url, sync_dir, &config_path())
}

#[tauri::command]
pub fn get_app_state() -> AppState {
    let config = Config::load(&config_path()).ok().flatten();
    app_state_from_config(config.as_ref())
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
}
