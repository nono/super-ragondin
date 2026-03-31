use std::path::Path;

use anyhow::{Context as _, Result};
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

    pub fn add_turn(&mut self, turn: Turn) {
        self.updated_at = Utc::now();
        self.turns.push(turn);
    }

    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created or the file cannot be written.
    pub fn save(&self, sessions_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(sessions_dir).context("failed to create sessions directory")?;
        let path = sessions_dir.join(format!("{}.json", self.id));
        let json = serde_json::to_string_pretty(self).context("failed to serialize session")?;
        std::fs::write(&path, json).context("failed to write session file")?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the file cannot be read or deserialized.
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path).context("failed to read session file")?;
        let session: Self = serde_json::from_str(&data).context("failed to deserialize session")?;
        Ok(session)
    }

    /// Find the most recently updated session within the timeout window.
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be read (except `NotFound`, which returns `Ok(None)`).
    pub fn find_recent(sessions_dir: &Path, timeout: std::time::Duration) -> Result<Option<Self>> {
        let entries = match std::fs::read_dir(sessions_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(anyhow::Error::new(e).context("failed to read sessions directory"));
            }
        };

        let cutoff = Utc::now()
            - chrono::Duration::from_std(timeout)
                .unwrap_or_else(|_| chrono::Duration::seconds(1800));

        let mut best: Option<Self> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(session) = Self::load(&path)
                && session.updated_at > cutoff
                && best
                    .as_ref()
                    .is_none_or(|b| session.updated_at > b.updated_at)
            {
                best = Some(session);
            }
        }
        Ok(best)
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

    #[test]
    fn add_turn_appends_and_updates_timestamp() {
        let mut session = Session::new("m", false);
        let before = session.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        let turn = Turn {
            timestamp: Utc::now(),
            question: "hello".to_string(),
            context_dir: None,
            tool_calls: vec![],
            interactions: vec![],
            answer: "world".to_string(),
        };
        session.add_turn(turn);
        assert_eq!(session.turns.len(), 1);
        assert!(session.updated_at >= before);
        assert_eq!(session.turns[0].question, "hello");
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new("test-model", true);
        session.add_turn(Turn {
            timestamp: Utc::now(),
            question: "q1".to_string(),
            context_dir: Some("work/docs".to_string()),
            tool_calls: vec![ToolCallRecord {
                code: "search('hi')".to_string(),
                result: "[]".to_string(),
            }],
            interactions: vec![InteractionRecord {
                question: "which?".to_string(),
                choices: vec!["a".to_string(), "b".to_string()],
                answer: "a".to_string(),
            }],
            answer: "a1".to_string(),
        });
        session.save(dir.path()).unwrap();
        let path = dir.path().join(format!("{}.json", session.id));
        assert!(path.exists());
        let loaded = Session::load(&path).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.turns.len(), 1);
        assert_eq!(loaded.turns[0].question, "q1");
        assert_eq!(loaded.turns[0].tool_calls.len(), 1);
        assert_eq!(loaded.turns[0].interactions.len(), 1);
        assert!(loaded.web_search);
    }

    #[test]
    fn find_recent_returns_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn find_recent_returns_none_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new("m", false);
        session.updated_at = Utc::now() - chrono::Duration::hours(1);
        session.save(dir.path()).unwrap();
        let result = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn find_recent_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let mut s1 = Session::new("m", false);
        s1.id = "2026-01-01T00-00-00Z".to_string();
        s1.save(dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let s2 = Session::new("m", false);
        s2.save(dir.path()).unwrap();
        let found = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800))
            .unwrap()
            .unwrap();
        assert_eq!(found.id, s2.id);
    }
}
