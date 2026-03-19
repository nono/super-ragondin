use std::path::PathBuf;
use super_ragondin_sync::config::Config;

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

#[tauri::command]
pub fn get_app_state() -> AppState {
    let config = Config::load(&config_path()).ok().flatten();
    app_state_from_config(config.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_ragondin_sync::remote::auth::OAuthClient;

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
