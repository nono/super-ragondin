use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `getDocument(docId)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("getDocument"),
        1,
        NativeFunction::from_fn_ptr(get_document_fn),
    )?;
    Ok(())
}

fn get_document_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let doc_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let chunks = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        sandbox.handle.block_on(async move {
            store
                .get_chunks(&doc_id)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    let json_chunks: Vec<serde_json::Value> = chunks
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "chunk_index": c.chunk_index,
                "chunk_text": c.chunk_text,
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_chunks), ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_document_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof getDocument"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
