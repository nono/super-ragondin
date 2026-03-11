use std::path::PathBuf;

pub const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const OPENROUTER_REFERER: &str = "https://github.com/super-ragondin";

#[derive(Clone)]
pub struct RagConfig {
    pub api_key: String,
    pub embed_model: String,
    pub vision_model: String,
    pub chat_model: String,
    pub subagent_model: String,
    pub db_path: PathBuf,
}

impl std::fmt::Debug for RagConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RagConfig")
            .field("api_key", &"***")
            .field("embed_model", &self.embed_model)
            .field("vision_model", &self.vision_model)
            .field("chat_model", &self.chat_model)
            .field("subagent_model", &self.subagent_model)
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl RagConfig {
    #[must_use]
    pub fn from_env_with_db_path(db_path: PathBuf) -> Self {
        Self {
            api_key: std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
            embed_model: std::env::var("OPENROUTER_EMBED_MODEL")
                .unwrap_or_else(|_| "openai/text-embedding-3-large".to_string()),
            vision_model: std::env::var("OPENROUTER_VISION_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
            chat_model: std::env::var("OPENROUTER_CHAT_MODEL")
                .unwrap_or_else(|_| "mistralai/mistral-small-3.2-24b-instruct".to_string()),
            subagent_model: std::env::var("OPENROUTER_SUBAGENT_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
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
                "OPENROUTER_SUBAGENT_MODEL",
            ],
            || {
                let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
                assert_eq!(config.embed_model, "openai/text-embedding-3-large");
                assert_eq!(config.vision_model, "google/gemini-2.5-flash");
                assert_eq!(
                    config.chat_model,
                    "mistralai/mistral-small-3.2-24b-instruct"
                );
                assert_eq!(config.subagent_model, "google/gemini-2.5-flash");
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
}
