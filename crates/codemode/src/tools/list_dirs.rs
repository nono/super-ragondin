use std::path::PathBuf;

use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use super::path_utils::check_relative_path;
use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `listDirs(prefix?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("listDirs"),
        0,
        NativeFunction::from_fn_ptr(list_dirs_fn),
    )?;
    Ok(())
}

fn list_dirs_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let prefix_val = args.get_or_undefined(0);
    let prefix = if prefix_val.is_undefined() || prefix_val.is_null() {
        String::new()
    } else {
        prefix_val.to_string(ctx)?.to_std_string_lossy()
    };

    if !prefix.is_empty()
        && let Err(msg) = check_relative_path(&prefix)
    {
        return Err(JsNativeError::error().with_message(msg).into());
    }

    let sync_dir = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<PathBuf, JsError>(sandbox.sync_dir.clone())
    })?;

    let target = if prefix.is_empty() {
        sync_dir
    } else {
        sync_dir.join(&prefix)
    };

    if !target.exists() {
        return serde_to_jsvalue(&serde_json::Value::Array(vec![]), ctx);
    }

    if !target.is_dir() {
        return Err(JsNativeError::error()
            .with_message("not a directory")
            .into());
    }

    let read_entries: Vec<_> = std::fs::read_dir(&target)
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e: std::io::Error| JsNativeError::error().with_message(e.to_string()))?;

    let mut names: Vec<String> = read_entries
        .into_iter()
        .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();

    names.sort();

    let json_names: Vec<serde_json::Value> =
        names.into_iter().map(serde_json::Value::String).collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_names), ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).expect("register should succeed");
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof listDirs"));
        assert_eq!(
            result
                .expect("eval should succeed")
                .as_string()
                .expect("result should be a string")
                .to_std_string_escaped(),
            "function"
        );
    }
}
