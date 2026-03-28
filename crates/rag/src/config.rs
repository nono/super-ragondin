use std::path::PathBuf;

pub const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const OPENROUTER_REFERER: &str = "https://github.com/super-ragondin";
pub const OPENROUTER_IMAGE_MODEL_DEFAULT: &str = "google/gemini-3.1-flash-image-preview";

#[derive(Clone)]
pub struct RagConfig {
    pub api_key: String,
    pub embed_model: String,
    pub vision_model: String,
    pub chat_model: String,
    pub subagent_model: String,
    pub image_model: String,
    pub db_path: PathBuf,
    pub api_url: String,
}

impl std::fmt::Debug for RagConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RagConfig")
            .field("api_key", &"***")
            .field("embed_model", &self.embed_model)
            .field("vision_model", &self.vision_model)
            .field("chat_model", &self.chat_model)
            .field("subagent_model", &self.subagent_model)
            .field("image_model", &self.image_model)
            .field("db_path", &self.db_path)
            .field("api_url", &self.api_url)
            .finish()
    }
}

impl RagConfig {
    /// Resolve an API key, preferring the `OPENROUTER_API_KEY` env var and
    /// falling back to `config_key`. Returns `None` if both are absent or empty.
    #[must_use]
    pub fn resolve_api_key(config_key: Option<&str>) -> Option<String> {
        std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| config_key.filter(|k| !k.is_empty()).map(str::to_owned))
    }

    #[must_use]
    pub fn from_env_with_db_path(db_path: PathBuf) -> Self {
        Self {
            api_key: std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
            embed_model: std::env::var("OPENROUTER_EMBED_MODEL")
                .unwrap_or_else(|_| "baai/bge-m3".to_string()),
            vision_model: std::env::var("OPENROUTER_VISION_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
            chat_model: std::env::var("OPENROUTER_CHAT_MODEL")
                .unwrap_or_else(|_| "mistralai/mistral-small-2603".to_string()),
            subagent_model: std::env::var("OPENROUTER_SUBAGENT_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
            image_model: std::env::var("OPENROUTER_IMAGE_MODEL")
                .unwrap_or_else(|_| OPENROUTER_IMAGE_MODEL_DEFAULT.to_string()),
            db_path,
            api_url: OPENROUTER_API_URL.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        temp_env::with_vars_unset(
            [
                "OPENROUTER_API_KEY",
                "OPENROUTER_EMBED_MODEL",
                "OPENROUTER_VISION_MODEL",
                "OPENROUTER_CHAT_MODEL",
                "OPENROUTER_SUBAGENT_MODEL",
                "OPENROUTER_IMAGE_MODEL",
            ],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.embed_model, "baai/bge-m3");
                assert_eq!(config.vision_model, "google/gemini-2.5-flash");
                assert_eq!(config.chat_model, "mistralai/mistral-small-2603");
                assert_eq!(config.subagent_model, "google/gemini-2.5-flash");
                assert_eq!(config.image_model, "google/gemini-3.1-flash-image-preview");
                assert!(config.api_key.is_empty());
            },
        );
    }

    #[test]
    fn test_config_from_env() {
        temp_env::with_vars(
            [
                ("OPENROUTER_API_KEY", Some("test-key")),
                ("OPENROUTER_EMBED_MODEL", Some("custom/model")),
            ],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.api_key, "test-key");
                assert_eq!(config.embed_model, "custom/model");
            },
        );
    }

    #[test]
    fn test_subagent_model_from_env() {
        temp_env::with_vars(
            [(
                "OPENROUTER_SUBAGENT_MODEL",
                Some("anthropic/claude-haiku-4-5"),
            )],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.subagent_model, "anthropic/claude-haiku-4-5");
            },
        );
    }

    #[test]
    fn test_image_model_from_env() {
        temp_env::with_vars(
            [("OPENROUTER_IMAGE_MODEL", Some("custom/img-model"))],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.image_model, "custom/img-model");
            },
        );
    }

    #[test]
    fn test_resolve_api_key_from_env() {
        temp_env::with_vars([("OPENROUTER_API_KEY", Some("env-key"))], || {
            assert_eq!(
                RagConfig::resolve_api_key(None),
                Some("env-key".to_string())
            );
        });
    }

    #[test]
    fn test_resolve_api_key_from_config_when_env_absent() {
        temp_env::with_vars_unset(["OPENROUTER_API_KEY"], || {
            assert_eq!(
                RagConfig::resolve_api_key(Some("config-key")),
                Some("config-key".to_string())
            );
        });
    }

    #[test]
    fn test_resolve_api_key_env_takes_priority_over_config() {
        temp_env::with_vars([("OPENROUTER_API_KEY", Some("env-key"))], || {
            assert_eq!(
                RagConfig::resolve_api_key(Some("config-key")),
                Some("env-key".to_string())
            );
        });
    }

    #[test]
    fn test_resolve_api_key_returns_none_when_both_absent() {
        temp_env::with_vars_unset(["OPENROUTER_API_KEY"], || {
            assert_eq!(RagConfig::resolve_api_key(None), None);
        });
    }

    #[test]
    fn test_resolve_api_key_ignores_empty_env_var() {
        temp_env::with_vars([("OPENROUTER_API_KEY", Some(""))], || {
            assert_eq!(
                RagConfig::resolve_api_key(Some("config-key")),
                Some("config-key".to_string())
            );
        });
    }

    #[test]
    fn test_resolve_api_key_ignores_empty_config_key() {
        temp_env::with_vars_unset(["OPENROUTER_API_KEY"], || {
            assert_eq!(RagConfig::resolve_api_key(Some("")), None);
        });
    }
}
