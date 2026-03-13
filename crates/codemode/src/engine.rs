use std::sync::Arc;

use anyhow::{Context as _, Result};
use super_ragondin_rag::{
    config::{OPENROUTER_API_URL, OPENROUTER_REFERER, RagConfig},
    store::RagStore,
};

use crate::prompt::system_prompt;
use crate::sandbox::Sandbox;

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
            "description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), and listDirs() functions to query the document database and write files.",
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

/// Extract the text content from an `OpenRouter` response, if present.
#[must_use]
pub(crate) fn extract_text(response: &serde_json::Value) -> Option<String> {
    response["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
}

/// Drives the code-mode ask loop for a single user question.
///
/// Stores the store `Arc` and config directly so it can cheaply clone them
/// into each `spawn_blocking` call (no unsafe needed).
pub struct CodeModeEngine {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
}

impl CodeModeEngine {
    /// Create a new engine, opening the RAG store.
    ///
    /// # Errors
    /// Returns error if the RAG store cannot be opened.
    pub async fn new(config: RagConfig, sync_dir: std::path::PathBuf) -> Result<Self> {
        let store = Arc::new(RagStore::open(&config.db_path).await?);
        Ok(Self {
            store,
            config,
            sync_dir,
        })
    }

    /// Ask a question using the code-mode LLM loop.
    ///
    /// Runs the tool-use loop (max `MAX_ITERATIONS` iterations), then prints the final answer.
    ///
    /// # Errors
    /// Returns error if the `OpenRouter` API call fails or the iteration limit is reached.
    pub async fn ask(&self, question: &str) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")?;
        let api_key = &self.config.api_key;
        let model = &self.config.chat_model;

        let mut messages = vec![
            serde_json::json!({"role": "system", "content": system_prompt()}),
            serde_json::json!({"role": "user", "content": question}),
        ];

        let tools = vec![execute_js_tool_definition()];

        for iteration in 0..MAX_ITERATIONS {
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
                messages.push(response["choices"][0]["message"].clone());

                // Execute all tool calls concurrently (spawn_blocking requires 'static).
                let mut handles = Vec::with_capacity(tool_calls.len());
                for tool_call in &tool_calls {
                    tracing::debug!(iteration, code = %tool_call.code, "execute_js tool call");
                    let store_clone = Arc::clone(&self.store);
                    let config_clone = self.config.clone();
                    let sync_dir_clone = self.sync_dir.clone();
                    let code_clone = tool_call.code.clone();
                    let id_clone = tool_call.id.clone();
                    handles.push(tokio::task::spawn_blocking(move || {
                        let sandbox = Sandbox::new(store_clone, config_clone, sync_dir_clone);
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
                println!("{text}");
                return Ok(());
            } else {
                anyhow::bail!("Unexpected response format from OpenRouter");
            }

            if iteration == MAX_ITERATIONS - 1 {
                anyhow::bail!(
                    "Reached maximum tool-call iterations ({MAX_ITERATIONS}) without a final answer"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
