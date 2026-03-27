use std::sync::Arc;

use anyhow::{Context as _, Result};
use super_ragondin_rag::{
    config::{OPENROUTER_API_URL, OPENROUTER_REFERER, RagConfig},
    store::RagStore,
};

use super_ragondin_cozy_client::client::CozyClient;

use crate::prompt::system_prompt;
use crate::sandbox::Sandbox;
use crate::tools::scratchpad::new_scratchpad;

const MAX_ITERATIONS: usize = 10;

/// Extracted tool call from an LLM response.
pub(crate) struct ToolCall {
    pub id: String,
    pub code: String,
}

/// Return the OpenAI-format tool definition for `execute_js`.
#[must_use]
pub(crate) fn execute_js_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "execute_js",
            "description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), listDirs(), generateImage(), remember(), recall(), and sendMail(subject, body) functions to query the document database, write files, generate images, store values across tool calls, and send emails to the user.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript code to execute. The last expression is returned as JSON."
                    }
                },
                "required": ["code"]
            }
        }
    })
}

/// Extract all `execute_js` tool calls from an `OpenRouter` response.
///
/// Returns an empty vec if there are no tool calls or none are `execute_js`.
#[must_use]
pub(crate) fn extract_tool_calls(response: &serde_json::Value) -> Vec<ToolCall> {
    let Some(tool_calls) = response["choices"][0]["message"]["tool_calls"].as_array() else {
        return vec![];
    };
    tool_calls
        .iter()
        .filter(|call| call["function"]["name"].as_str() == Some("execute_js"))
        .filter_map(|call| {
            let args_str = call["function"]["arguments"].as_str()?;
            let args: serde_json::Value = serde_json::from_str(args_str).ok()?;
            Some(ToolCall {
                id: call["id"].as_str()?.to_string(),
                code: args["code"].as_str()?.to_string(),
            })
        })
        .collect()
}

use crate::llm::extract_text;

/// Drives the code-mode ask loop for a single user question.
///
/// Stores the store `Arc` and config directly so it can cheaply clone them
/// into each `spawn_blocking` call (no unsafe needed).
pub struct CodeModeEngine {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
    cozy_client: Option<Arc<CozyClient>>,
}

impl CodeModeEngine {
    /// Create a new engine, opening the RAG store.
    ///
    /// # Errors
    /// Returns error if the RAG store cannot be opened.
    pub async fn new(
        config: RagConfig,
        sync_dir: std::path::PathBuf,
        cozy_client: Option<Arc<CozyClient>>,
    ) -> Result<Self> {
        let store = Arc::new(RagStore::open(&config.db_path).await?);
        Ok(Self {
            store,
            config,
            sync_dir,
            cozy_client,
        })
    }

    /// Ask a question using the code-mode LLM loop.
    ///
    /// Runs the tool-use loop (max `MAX_ITERATIONS` iterations), then returns the final answer.
    ///
    /// # Errors
    /// Returns error if the `OpenRouter` API call fails or the iteration limit is reached.
    pub async fn ask(
        &self,
        question: &str,
        context_dir: Option<std::path::PathBuf>,
    ) -> Result<String> {
        tracing::info!(question, "✦ Ask: engine loop starting");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")?;
        let api_key = &self.config.api_key;
        let model = &self.config.chat_model;

        let context_msg = self.build_context_message(context_dir).await;

        let mut messages =
            vec![serde_json::json!({"role": "system", "content": system_prompt(false)})];
        if let Some(ctx) = context_msg {
            messages.push(serde_json::json!({"role": "user", "content": ctx}));
        }
        messages.push(serde_json::json!({"role": "user", "content": question}));

        let tools = vec![execute_js_tool_definition()];
        let scratchpad = new_scratchpad();

        for iteration in 0..MAX_ITERATIONS {
            tracing::debug!(
                iteration,
                model = model.as_str(),
                "✦ Ask: sending request to OpenRouter"
            );
            let body = serde_json::json!({
                "model": model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto"
            });

            let resp = client
                .post(OPENROUTER_API_URL)
                .bearer_auth(api_key)
                .header("HTTP-Referer", OPENROUTER_REFERER)
                .json(&body)
                .send()
                .await
                .context("OpenRouter request failed")?;

            let response: serde_json::Value =
                resp.json().await.context("Failed to parse response")?;

            let tool_calls = extract_tool_calls(&response);
            if !tool_calls.is_empty() {
                tracing::info!(
                    iteration,
                    count = tool_calls.len(),
                    "✦ Ask: executing tool calls"
                );
                messages.push(response["choices"][0]["message"].clone());

                // Execute all tool calls concurrently (spawn_blocking requires 'static).
                let mut handles = Vec::with_capacity(tool_calls.len());
                for tool_call in &tool_calls {
                    tracing::debug!(iteration, code = %tool_call.code, "execute_js tool call");
                    let store_clone = Arc::clone(&self.store);
                    let config_clone = self.config.clone();
                    let sync_dir_clone = self.sync_dir.clone();
                    let scratchpad_clone = Arc::clone(&scratchpad);
                    let cozy_client_clone = self.cozy_client.clone();
                    let code_clone = tool_call.code.clone();
                    let id_clone = tool_call.id.clone();
                    handles.push(tokio::task::spawn_blocking(move || {
                        let sandbox = Sandbox::new(
                            store_clone,
                            config_clone,
                            sync_dir_clone,
                            scratchpad_clone,
                            cozy_client_clone,
                            None,
                        );
                        (id_clone, sandbox.execute(&code_clone))
                    }));
                }

                for handle in handles {
                    let (id, result) = handle.await.context("spawn_blocking panicked")?;
                    let tool_result = match result {
                        Ok(json_str) => json_str,
                        Err(err_msg) => err_msg, // JS errors returned as strings for LLM to adapt
                    };
                    tracing::debug!(result = %tool_result, "execute_js result");
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": tool_result
                    }));
                }
            } else if let Some(text) = extract_text(&response) {
                tracing::info!(
                    iteration,
                    answer_len = text.len(),
                    "✦ Ask: finished with answer"
                );
                return Ok(text);
            } else {
                tracing::error!(iteration, response = %response, "✦ Ask: unexpected response format");
                anyhow::bail!("Unexpected response format from OpenRouter");
            }

            if iteration == MAX_ITERATIONS - 1 {
                tracing::error!("✦ Ask: reached maximum iterations ({MAX_ITERATIONS})");
                anyhow::bail!(
                    "Reached maximum tool-call iterations ({MAX_ITERATIONS}) without a final answer"
                );
            }
        }

        unreachable!()
    }

    /// Build the optional context message to prepend before the user question.
    ///
    /// Returns `None` if there are no signals to report (no CWD inside `sync_dir`
    /// and no recently modified files).
    async fn build_context_message(
        &self,
        context_dir: Option<std::path::PathBuf>,
    ) -> Option<String> {
        // Compute relative CWD if inside sync_dir
        let relative_cwd: Option<String> = context_dir.and_then(|dir| {
            dir.strip_prefix(&self.sync_dir)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });

        // Query recent files (last 15 minutes)
        let since = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(900))
            .unwrap_or(std::time::UNIX_EPOCH);
        let recent_files = self.store.list_recent(since).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "list_recent failed, skipping recent files context");
            vec![]
        });

        if relative_cwd.is_none() && recent_files.is_empty() {
            return None;
        }

        let mut lines = vec!["[Context]".to_string()];
        if let Some(ref cwd) = relative_cwd {
            let display = if cwd.is_empty() { "." } else { cwd.as_str() };
            lines.push(format!("Current directory: {display}"));
        }
        if !recent_files.is_empty() {
            lines.push("Recently modified (last 15 min):".to_string());
            for path in &recent_files {
                lines.push(format!("- {path}"));
            }
        }
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helpers for build_context_message tests
    async fn make_engine_for_ctx_test() -> (CodeModeEngine, tempfile::TempDir, tempfile::TempDir) {
        use std::sync::Arc;
        use super_ragondin_rag::{config::RagConfig, store::RagStore};
        let db_dir = tempfile::tempdir().expect("db_dir");
        let sync_dir = tempfile::tempdir().expect("sync_dir");
        let store = RagStore::open(db_dir.path()).await.expect("store");
        let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
        let engine = CodeModeEngine {
            store: Arc::new(store),
            config,
            sync_dir: sync_dir.path().to_path_buf(),
            cozy_client: None,
        };
        (engine, db_dir, sync_dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_none_when_no_signals() {
        let (engine, _db, _sync) = make_engine_for_ctx_test().await;
        let result = engine.build_context_message(None).await;
        assert!(result.is_none(), "should be None when no signals");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_includes_relative_dir() {
        let (engine, _db, sync) = make_engine_for_ctx_test().await;
        let sub = sync.path().join("work/meetings");
        std::fs::create_dir_all(&sub).unwrap();
        let result = engine.build_context_message(Some(sub)).await;
        let msg = result.expect("should have message");
        assert!(msg.contains("Current directory:"), "got: {msg}");
        assert!(msg.contains("work/meetings"), "got: {msg}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_outside_sync_dir_no_cwd_line() {
        let (engine, _db, _sync) = make_engine_for_ctx_test().await;
        let outside = tempfile::tempdir().unwrap();
        let result = engine
            .build_context_message(Some(outside.path().to_path_buf()))
            .await;
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_recent_files_no_cwd() {
        use std::time::UNIX_EPOCH;
        use super_ragondin_rag::store::ChunkRecord;
        let (engine, _db, _sync) = make_engine_for_ctx_test().await;
        let now = std::time::SystemTime::now();
        let recent_secs = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .cast_signed()
            - 60;
        let chunk = ChunkRecord {
            id: "docs/recent.md-0".to_string(),
            doc_id: "docs/recent.md".to_string(),
            mime_type: "text/plain".to_string(),
            mtime: recent_secs,
            chunk_index: 0,
            chunk_text: "hello".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 1024],
        };
        engine.store.upsert_chunks(&[chunk]).await.unwrap();

        let result = engine.build_context_message(None).await;
        let msg = result.expect("should have message when recent files exist");
        assert!(msg.contains("Recently modified"), "got: {msg}");
        assert!(msg.contains("docs/recent.md"), "got: {msg}");
        assert!(
            !msg.contains("Current directory:"),
            "should not have CWD line; got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_context_message_starts_with_context_header() {
        let (engine, _db, sync) = make_engine_for_ctx_test().await;
        let sub = sync.path().join("notes");
        std::fs::create_dir_all(&sub).unwrap();
        let ctx_msg = engine.build_context_message(Some(sub)).await;
        assert!(ctx_msg.is_some());
        let msg = ctx_msg.unwrap();
        assert!(msg.starts_with("[Context]"), "got: {msg}");
    }

    #[test]
    fn test_execute_js_tool_definition() {
        let def = execute_js_tool_definition();
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], "execute_js");
        assert!(def["function"]["parameters"]["properties"]["code"].is_object());
    }

    #[test]
    fn test_extract_tool_calls_single() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "execute_js",
                            "arguments": "{\"code\": \"1 + 1\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let calls = extract_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].code, "1 + 1");
    }

    #[test]
    fn test_extract_tool_calls_multiple() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "execute_js",
                                "arguments": "{\"code\": \"1 + 1\"}"
                            }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": {
                                "name": "execute_js",
                                "arguments": "{\"code\": \"2 + 2\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let calls = extract_tool_calls(&response);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn test_extract_text_from_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42."
                },
                "finish_reason": "stop"
            }]
        });
        assert!(extract_tool_calls(&response).is_empty());
        let text = extract_text(&response).unwrap();
        assert_eq!(text, "The answer is 42.");
    }
}
