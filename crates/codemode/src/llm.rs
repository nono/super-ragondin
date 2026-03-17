use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

/// Call the OpenRouter chat completions endpoint.
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
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

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
