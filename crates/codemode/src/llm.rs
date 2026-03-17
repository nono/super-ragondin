use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

/// Extract the text content from an `OpenRouter` response, if present.
#[must_use]
pub fn extract_text(response: &serde_json::Value) -> Option<String> {
    response["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
}

/// Call the `OpenRouter` chat completions endpoint.
///
/// `messages` is a list of objects with `role` and `content` fields.
///
/// # Errors
/// Returns an error if the HTTP call fails or the response cannot be parsed.
pub async fn call_llm(
    api_key: &str,
    model: &str,
    messages: Vec<serde_json::Value>,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
    });
    let resp = client
        .post(OPENROUTER_API_URL)
        .bearer_auth(api_key)
        .header("HTTP-Referer", OPENROUTER_REFERER)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    let json: serde_json::Value = resp.json().await?;
    if let Some(msg) = json["error"]["message"].as_str() {
        anyhow::bail!("OpenRouter error: {msg}");
    }
    extract_text(&json).ok_or_else(|| anyhow::anyhow!("missing content in OpenRouter response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_present() {
        let json = serde_json::json!({"choices": [{"message": {"content": "hello"}}]});
        assert_eq!(extract_text(&json).as_deref(), Some("hello"));
    }

    #[test]
    fn test_extract_text_missing() {
        let json = serde_json::json!({"error": {"message": "rate limit exceeded"}});
        assert!(extract_text(&json).is_none());
    }

    #[test]
    fn test_call_llm_signature_compiles() {
        fn _check_type<'a>(
            api_key: &'a str,
            model: &'a str,
            messages: Vec<serde_json::Value>,
        ) -> impl std::future::Future<Output = anyhow::Result<String>> + 'a {
            call_llm(api_key, model, messages)
        }
        let _ = _check_type;
    }
}
