use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use super::path_utils::check_relative_path;
use crate::sandbox::SANDBOX_CTX;

/// Register the `saveFile(path, content, options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("saveFile"),
        0,
        NativeFunction::from_fn_ptr(save_file_fn),
    )?;
    Ok(())
}

fn save_file_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let path = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let content = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_lossy();

    let encoding = args
        .get(2)
        .and_then(|opts| opts.as_object())
        .and_then(|o| o.get(boa_engine::JsString::from("encoding"), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map_or_else(|| "utf8".to_string(), |s| s.to_std_string_escaped());

    if Path::new(&path).is_absolute() {
        return Err(JsNativeError::error()
            .with_message("path must be relative")
            .into());
    }

    if let Err(msg) = check_relative_path(&path) {
        return Err(JsNativeError::error().with_message(msg).into());
    }

    let sync_dir = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<PathBuf, JsError>(sandbox.sync_dir.clone())
    })?;

    let target = sync_dir.join(&path);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
    }

    let bytes: Vec<u8> = if encoding == "base64" {
        STANDARD
            .decode(&content)
            .map_err(|_| JsNativeError::error().with_message("invalid base64 content"))?
    } else {
        content.into_bytes()
    };

    std::fs::write(&target, &bytes)
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;

    Ok(JsValue::undefined())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof saveFile"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
