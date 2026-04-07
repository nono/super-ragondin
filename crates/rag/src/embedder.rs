use crate::config::RagConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait VisionDescriber: Send + Sync {
    /// Call vision LLM to describe an image. `image_b64` is base64-encoded bytes.
    /// `mime` is e.g. "image/png".
    async fn describe_image(&self, image_b64: &str, mime: &str) -> Result<String>;
}

pub struct OpenRouterVision {
    client: reqwest::Client,
    config: RagConfig,
}

impl OpenRouterVision {
    #[must_use]
    pub fn new(config: RagConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: String,
}

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
const MAX_RETRIES: u32 = 3;

#[allow(clippy::future_not_send)]
async fn post_with_retry(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response> {
    let mut delay_ms = 500u64;
    let mut last_err: anyhow::Error = anyhow::anyhow!("OpenRouter: exhausted retries");
    for attempt in 0..MAX_RETRIES {
        let resp = match client
            .post(url)
            .bearer_auth(api_key)
            .header("HTTP-Referer", "https://github.com/super-ragondin")
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = e.into();
                if attempt + 1 < MAX_RETRIES {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                }
                continue;
            }
        };
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if status.as_u16() == 429 || status.is_server_error() {
            if attempt + 1 < MAX_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 2;
            }
            last_err = anyhow::anyhow!("OpenRouter error {status}");
            continue;
        }
        return Err(anyhow::anyhow!(
            "OpenRouter error {status}: {}",
            resp.text().await?
        ));
    }
    Err(last_err)
}

#[async_trait]
impl VisionDescriber for OpenRouterVision {
    async fn describe_image(&self, image_b64: &str, mime: &str) -> Result<String> {
        let data_url = format!("data:{mime};base64,{image_b64}");
        let body = ChatRequest {
            model: &self.config.vision_model,
            messages: vec![ChatMessage {
                role: "user",
                content: serde_json::json!([
                    {
                        "type": "image_url",
                        "image_url": { "url": data_url }
                    },
                    {
                        "type": "text",
                        "text": "Describe the content of this image in detail, in the language of the text it contains if any. Focus on information that would be useful for search and retrieval."
                    }
                ]),
            }],
            stream: false,
        };
        let resp = post_with_retry(
            &self.client,
            &format!("{OPENROUTER_BASE}/chat/completions"),
            &self.config.api_key,
            &body,
        )
        .await?;
        let parsed: ChatResponse = resp.json().await?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubDescriber;

    #[async_trait::async_trait]
    impl VisionDescriber for StubDescriber {
        async fn describe_image(&self, _image_b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("a test image".to_string())
        }
    }

    #[tokio::test]
    async fn test_stub_describe_image() -> anyhow::Result<()> {
        let d = StubDescriber;
        let result = d.describe_image("base64data", "image/png").await?;
        assert_eq!(result, "a test image");
        Ok(())
    }
}
