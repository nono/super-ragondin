use std::sync::OnceLock;

use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::SANDBOX_CTX;

const TIMEOUT_SECS: u64 = 30;
const MAX_BODY_BYTES: usize = 1_048_576; // 1 MB

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .user_agent("SuperRagondin/0.1")
            .build()
            .expect("failed to build reqwest client")
    })
}

/// Register the `webFetch(url)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("webFetch"),
        1,
        NativeFunction::from_fn_ptr(web_fetch_fn),
    )?;
    Ok(())
}

fn web_fetch_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let url = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let handle = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<_, JsError>(sandbox.handle.clone())
    })?;

    let result = handle.block_on(async {
        let resp = http_client()
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let is_text = content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("javascript");

        let body = if is_text {
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
            let truncated = &bytes[..bytes.len().min(MAX_BODY_BYTES)];
            String::from_utf8_lossy(truncated).into_owned()
        } else {
            String::new()
        };

        Ok::<_, String>(serde_json::json!({
            "status": status,
            "contentType": content_type,
            "body": body,
        }))
    });

    match result {
        Ok(json_val) => crate::sandbox::serde_to_jsvalue(&json_val, ctx),
        Err(e) => Err(JsNativeError::error().with_message(e).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof webFetch"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_web_fetch_returns_status_and_body() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("hello world")
                    .insert_header("content-type", "text/plain"),
            )
            .mount(&mock_server)
            .await;

        let (sandbox, _db, _sync) = crate::sandbox::tests::make_sandbox().await;
        let uri = mock_server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let code = format!(r#"webFetch("{uri}")"#);
            sandbox.execute(&code)
        })
        .await
        .expect("spawn_blocking panicked")
        .expect("execute failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], 200);
        assert_eq!(parsed["contentType"], "text/plain");
        assert_eq!(parsed["body"], "hello world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_web_fetch_binary_returns_empty_body() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(vec![0x89, 0x50, 0x4E, 0x47])
                    .insert_header("content-type", "image/png"),
            )
            .mount(&mock_server)
            .await;

        let (sandbox, _db, _sync) = crate::sandbox::tests::make_sandbox().await;
        let uri = mock_server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let code = format!(r#"webFetch("{uri}")"#);
            sandbox.execute(&code)
        })
        .await
        .expect("spawn_blocking panicked")
        .expect("execute failed");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["status"], 200);
        assert_eq!(parsed["body"], "");
    }
}
