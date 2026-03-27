use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use super::path_utils::check_relative_path;
use crate::sandbox::SANDBOX_CTX;

/// Register the `mkdir(path)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("mkdir"),
        0,
        NativeFunction::from_fn_ptr(mkdir_fn),
    )?;
    Ok(())
}

fn mkdir_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;
    use std::path::PathBuf;

    let path = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_lossy();

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
    std::fs::create_dir_all(&target)
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;

    Ok(JsValue::undefined())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).expect("register should succeed");
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof mkdir"));
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
