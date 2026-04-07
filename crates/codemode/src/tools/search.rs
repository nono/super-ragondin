use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::store::MetadataFilter;

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `search(query, options?)` global function in the Boa context.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("search"),
        1,
        NativeFunction::from_fn_ptr(search_fn),
    )?;
    Ok(())
}

fn search_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let query = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    // Parse options object
    let opts = args.get(1).cloned().unwrap_or(JsValue::undefined());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let limit = get_number_opt(&opts, "limit", ctx).map_or(5, |n| n as usize);
    let mime_type = get_string_opt(&opts, "mimeType", ctx);
    let path_prefix = get_string_opt(&opts, "pathPrefix", ctx);
    let after = get_string_opt(&opts, "after", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());
    let before = get_string_opt(&opts, "before", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());

    let filter = MetadataFilter {
        mime_type,
        path_prefix,
        after,
        before,
    };
    let has_filter = filter.mime_type.is_some()
        || filter.path_prefix.is_some()
        || filter.after.is_some()
        || filter.before.is_some();

    let results = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        let filter_opt = if has_filter { Some(filter) } else { None };
        store
            .search(&query, limit, filter_opt.as_ref())
            .map_err(|e| JsNativeError::error().with_message(e.to_string()))
    })?;

    // Convert to JS: [{ doc_id, chunk_text, mime_type, mtime }]
    let json_results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            use chrono::{TimeZone, Utc};
            let mtime_dt = Utc
                .timestamp_opt(r.mtime, 0)
                .single()
                .unwrap_or_else(Utc::now);
            serde_json::json!({
                "doc_id": r.doc_id,
                "chunk_text": r.chunk_text,
                "mime_type": r.mime_type,
                "mtime": mtime_dt.to_rfc3339(),
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_results), ctx)
}

fn get_string_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<String> {
    obj.as_object()
        .and_then(|o| o.get(boa_engine::JsString::from(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
}

fn get_number_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<f64> {
    obj.as_object()
        .and_then(|o| o.get(boa_engine::JsString::from(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_number(ctx).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof search"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
