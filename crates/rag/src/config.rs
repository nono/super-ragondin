use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RagConfig {
    pub api_key: String,
    pub embed_model: String,
    pub vision_model: String,
    pub chat_model: String,
    pub db_path: PathBuf,
}

impl RagConfig {
    pub fn from_env_with_db_path(db_path: PathBuf) -> Self {
        Self {
            api_key: std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
            embed_model: std::env::var("OPENROUTER_EMBED_MODEL")
                .unwrap_or_else(|_| "openai/text-embedding-3-large".to_string()),
            vision_model: std::env::var("OPENROUTER_VISION_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.0-flash".to_string()),
            chat_model: std::env::var("OPENROUTER_CHAT_MODEL")
                .unwrap_or_else(|_| "mistralai/mistral-small-3.2-24b-instruct".to_string()),
            db_path,
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
            ],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.embed_model, "openai/text-embedding-3-large");
                assert_eq!(config.vision_model, "google/gemini-2.0-flash");
                assert_eq!(
                    config.chat_model,
                    "mistralai/mistral-small-3.2-24b-instruct"
                );
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
}
