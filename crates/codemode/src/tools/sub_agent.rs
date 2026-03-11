use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

use crate::sandbox::SANDBOX_CTX;

/// Register the `subAgent(systemPrompt, userPrompt)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("subAgent"),
        2,
        NativeFunction::from_fn_ptr(sub_agent_fn),
    )?;
    Ok(())
}

fn sub_agent_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let system_prompt = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let user_prompt = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();

    let response = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let api_key = sandbox.config.api_key.clone();
        let model = sandbox.config.subagent_model.clone();
        sandbox.handle.block_on(async move {
            call_sub_agent(&api_key, &model, &system_prompt, &user_prompt)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    Ok(JsValue::String(boa_engine::JsString::from(
        response.as_str(),
    )))
}

async fn call_sub_agent(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]
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
    fn test_sub_agent_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof subAgent"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
