use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super_ragondin_codemode::CodeModeEngine;
use super_ragondin_codemode::interaction::UserInteraction;
use super_ragondin_rag::{config::RagConfig, store::RagStore};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A fake interaction backend that records whether it was called and returns a fixed answer.
struct FakeInteraction {
    called: AtomicBool,
    answer: String,
}

impl FakeInteraction {
    fn new(answer: &str) -> Self {
        Self {
            called: AtomicBool::new(false),
            answer: answer.to_string(),
        }
    }

    fn was_called(&self) -> bool {
        self.called.load(Ordering::SeqCst)
    }
}

impl UserInteraction for FakeInteraction {
    fn ask(&self, _question: &str, _choices: &[&str]) -> String {
        self.called.store(true, Ordering::SeqCst);
        self.answer.clone()
    }
}

/// Simulates a full engine `ask()` loop where the LLM calls `askUser()` for clarification,
/// gets the user's answer, and produces a final response incorporating that answer.
#[tokio::test(flavor = "multi_thread")]
async fn test_engine_ask_with_user_clarification() {
    let mock_server = MockServer::start().await;

    // First LLM response: call askUser via execute_js
    let tool_call_response = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_clarify",
                    "type": "function",
                    "function": {
                        "name": "execute_js",
                        "arguments": r#"{"code": "askUser(\"Which format do you prefer?\", [\"PDF\", \"Markdown\"])"}"#
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    // Second LLM response: final text answer using the user's choice
    let final_response = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I'll create the report in Markdown format as you requested."
            },
            "finish_reason": "stop"
        }]
    });

    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&tool_call_response))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&final_response))
        .expect(1)
        .mount(&mock_server)
        .await;

    let db_dir = tempfile::tempdir().expect("db_dir");
    let sync_dir = tempfile::tempdir().expect("sync_dir");
    let store = Arc::new(
        RagStore::open(db_dir.path())
            .await
            .expect("failed to open store"),
    );
    let mut config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    config.api_key = "test-key".to_string();
    config.api_url = format!("{}/api/v1/chat/completions", mock_server.uri());

    let interaction = Arc::new(FakeInteraction::new("Markdown"));
    let engine = CodeModeEngine::new_with_store(
        store,
        config,
        sync_dir.path().to_path_buf(),
        None,
        Some(interaction.clone() as Arc<dyn UserInteraction>),
    );

    let answer = engine
        .ask(
            "Generate a report about recent documents",
            None,
            false,
            true,
        )
        .await
        .expect("ask should succeed");

    assert!(
        interaction.was_called(),
        "askUser interaction should have been called"
    );
    assert!(
        answer.contains("Markdown"),
        "final answer should mention the user's choice; got: {answer}"
    );
}
