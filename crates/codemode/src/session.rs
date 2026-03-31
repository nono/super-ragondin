use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub code: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRecord {
    pub question: String,
    pub choices: Vec<String>,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub timestamp: DateTime<Utc>,
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_dir: Option<String>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub interactions: Vec<InteractionRecord>,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub web_search: bool,
    pub turns: Vec<Turn>,
}

impl Session {
    #[must_use]
    pub fn new(model: &str, web_search: bool) -> Self {
        let now = Utc::now();
        let id = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
        Self {
            id,
            created_at: now,
            updated_at: now,
            model: model.to_string(),
            web_search,
            turns: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_has_empty_turns() {
        let session = Session::new("test-model", false);
        assert!(session.turns.is_empty());
        assert_eq!(session.model, "test-model");
        assert!(!session.web_search);
    }

    #[test]
    fn new_session_id_is_timestamp_format() {
        let session = Session::new("m", false);
        assert!(session.id.ends_with('Z'), "id={}", session.id);
        assert!(session.id.len() >= 20, "id={}", session.id);
    }
}
