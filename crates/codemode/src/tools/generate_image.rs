use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

use crate::sandbox::SANDBOX_CTX;
use crate::tools::path_utils::check_relative_path;

/// Register the `generateImage(prompt, options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("generateImage"),
        1,
        NativeFunction::from_fn_ptr(generate_image_fn),
    )?;
    Ok(())
}

fn generate_image_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;
    // Parse prompt (required)
    let prompt_val = args.get_or_undefined(0);
    if prompt_val.is_undefined() || prompt_val.is_null() {
        return Err(JsNativeError::error()
            .with_message("prompt is required")
            .into());
    }
    let prompt = prompt_val.to_string(ctx)?.to_std_string_escaped();
    if prompt.is_empty() {
        return Err(JsNativeError::error()
            .with_message("prompt is required")
            .into());
    }

    // Parse options object
    let opts = args.get(1).and_then(|v| v.as_object().cloned());

    let path_opt = get_string_option(opts.as_ref(), "path", ctx)?;
    let reference_opt = get_string_option(opts.as_ref(), "reference", ctx)?;
    let aspect =
        get_string_option(opts.as_ref(), "aspect", ctx)?.unwrap_or_else(|| "1:1".to_string());
    let size = get_string_option(opts.as_ref(), "size", ctx)?.unwrap_or_else(|| "0.5K".to_string());

    // Validate paths early — before any I/O or SANDBOX_CTX access
    if let Some(p) = &path_opt {
        if Path::new(p).is_absolute() {
            return Err(JsNativeError::error()
                .with_message("path must be relative")
                .into());
        }
        check_relative_path(p).map_err(|e| JsNativeError::error().with_message(e))?;
    }
    if let Some(r) = &reference_opt {
        if Path::new(r).is_absolute() {
            return Err(JsNativeError::error()
                .with_message("path must be relative")
                .into());
        }
        check_relative_path(r).map_err(|e| JsNativeError::error().with_message(e))?;
    }

    // Read sandbox context
    let (api_key, image_model, handle, sync_dir) = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<_, JsError>((
            sandbox.config.api_key.clone(),
            sandbox.config.image_model.clone(),
            sandbox.handle.clone(),
            sandbox.sync_dir.clone(),
        ))
    })?;

    // If reference given, read file and encode as base64 data URL
    let reference_data_url: Option<String> = if let Some(r) = reference_opt {
        let ref_path = sync_dir.join(&r);
        let bytes = std::fs::read(&ref_path)
            .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
        let mime = infer::get(&bytes)
            .map_or_else(|| "image/png".to_string(), |t| t.mime_type().to_string());
        Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
    } else {
        None
    };

    // Compute absolute save path (if requested)
    let save_path: Option<PathBuf> = path_opt.map(|p| sync_dir.join(p));

    // Values are cloned out of SANDBOX_CTX before block_on so the RefCell borrow
    // is released before the async work begins (avoids holding the borrow across await).
    let b64 = handle.block_on(async move {
        generate_image_async(
            &api_key,
            &image_model,
            &prompt,
            &aspect,
            &size,
            reference_data_url.as_deref(),
            save_path.as_deref(),
        )
        .await
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))
    })?;

    Ok(JsValue::String(boa_engine::JsString::from(b64.as_str())))
}

/// Extract an optional string field from a Boa options object.
fn get_string_option(
    opts: Option<&boa_engine::object::JsObject>,
    key: &str,
    ctx: &mut Context,
) -> JsResult<Option<String>> {
    let Some(obj) = opts else { return Ok(None) };
    let val = obj
        .get(boa_engine::JsString::from(key), ctx)
        .unwrap_or(JsValue::undefined());
    if val.is_undefined() || val.is_null() {
        return Ok(None);
    }
    Ok(Some(val.to_string(ctx)?.to_std_string_escaped()))
}

async fn generate_image_async(
    api_key: &str,
    model: &str,
    prompt: &str,
    aspect: &str,
    size: &str,
    reference_data_url: Option<&str>,
    save_path: Option<&Path>,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    // Build message content array
    let mut content: Vec<serde_json::Value> = Vec::new();
    if let Some(ref_url) = reference_data_url {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": ref_url}
        }));
    }
    content.push(serde_json::json!({"type": "text", "text": prompt}));

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "modalities": ["image", "text"],
        "image_config": {
            "aspect_ratio": aspect,
            "image_size": size
        }
    });

    let resp = client
        .post(OPENROUTER_API_URL)
        .bearer_auth(api_key)
        .header("HTTP-Referer", OPENROUTER_REFERER)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenRouter error {status}: {body_text}");
    }

    let json: serde_json::Value = resp.json().await?;

    // Extract image URL from the non-standard `images` field
    let images = json["choices"][0]["message"]["images"].as_array();
    let image_url = images
        .and_then(|arr| arr.first())
        .and_then(|img| img["image_url"]["url"].as_str());

    let url = if let Some(u) = image_url {
        u.to_string()
    } else {
        let msg_content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        if msg_content.is_empty() {
            anyhow::bail!("no image returned by model");
        }
        anyhow::bail!("no image returned by model: {msg_content}");
    };

    // Decode image to raw bytes
    let bytes: Vec<u8> = if url.starts_with("data:image/") {
        let comma = url
            .find(',')
            .ok_or_else(|| anyhow::anyhow!("invalid base64 in response"))?;
        let b64 = &url[comma + 1..];
        STANDARD
            .decode(b64)
            .map_err(|_| anyhow::anyhow!("invalid base64 in response"))?
    } else {
        // Plain HTTPS URL — fetch with same client (120 s timeout inherited)
        client.get(&url).send().await?.bytes().await?.to_vec()
    };

    // Write to file if a save path was given
    if let Some(path) = save_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &bytes)?;
    }

    Ok(STANDARD.encode(&bytes))
}

#[cfg(test)]
mod tests {
    use boa_engine::{Context, Source};

    fn register_fn(ctx: &mut Context) {
        super::register(ctx).unwrap();
    }

    #[test]
    fn test_registers_as_function() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"typeof generateImage"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_no_args_returns_error_about_prompt() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"generateImage()"));
        assert!(result.is_err());
        let err_str = format!("{:?}", result.unwrap_err());
        assert!(
            err_str.contains("prompt"),
            "error should mention 'prompt', got: {err_str}"
        );
    }

    #[test]
    fn test_path_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_path_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "/absolute/path.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "/absolute/ref.png" })"#,
        ));
        assert!(result.is_err());
    }
}
