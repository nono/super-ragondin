use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::store::{DocSort, MetadataFilter};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `listFiles(options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("listFiles"),
        0,
        NativeFunction::from_fn_ptr(list_files_fn),
    )?;
    Ok(())
}

fn list_files_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let opts = args.first().cloned().unwrap_or(JsValue::undefined());

    let sort_str = get_string_opt(&opts, "sort", ctx).unwrap_or_else(|| "recent".to_string());
    let sort = if sort_str == "oldest" {
        DocSort::Oldest
    } else {
        DocSort::Recent
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let limit = get_number_opt(&opts, "limit", ctx).map(|n| n as usize);
    let mime_type = get_string_opt(&opts, "mimeType", ctx);
    let path_prefix = get_string_opt(&opts, "pathPrefix", ctx);
    let after = get_string_opt(&opts, "after", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());
    let before = get_string_opt(&opts, "before", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());

    let has_filter =
        mime_type.is_some() || path_prefix.is_some() || after.is_some() || before.is_some();
    let filter = MetadataFilter {
        mime_type,
        path_prefix,
        after,
        before,
    };

    let docs = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        let filter_opt = if has_filter { Some(filter) } else { None };
        sandbox.handle.block_on(async move {
            store
                .list_docs(filter_opt.as_ref(), sort, limit)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    let json_docs: Vec<serde_json::Value> = docs
        .into_iter()
        .map(|d| {
            use chrono::{TimeZone, Utc};
            let mtime_dt = Utc
                .timestamp_opt(d.mtime, 0)
                .single()
                .unwrap_or_else(Utc::now);
            serde_json::json!({
                "doc_id": d.doc_id,
                "mime_type": d.mime_type,
                "mtime": mtime_dt.to_rfc3339(),
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_docs), ctx)
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
    fn test_list_files_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof listFiles"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
