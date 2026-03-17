use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use serde_json::Value as SerdeValue;

use crate::sandbox::{SANDBOX_CTX, jsvalue_to_serde, serde_to_jsvalue};

/// Shared in-session key-value store, valid for one `ask()` call.
pub type Scratchpad = Arc<Mutex<HashMap<String, SerdeValue>>>;

/// Create a fresh, empty scratchpad for a new `ask()` session.
#[must_use]
pub fn new_scratchpad() -> Scratchpad {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Register `remember(key, value)` and `recall(key)` as JS globals.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("remember"),
        2,
        NativeFunction::from_fn_ptr(remember_fn),
    )?;
    ctx.register_global_callable(
        js_string!("recall"),
        1,
        NativeFunction::from_fn_ptr(recall_fn),
    )?;
    Ok(())
}

fn remember_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let key = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let value = jsvalue_to_serde(args.get_or_undefined(1).clone(), ctx);

    let scratchpad = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<Scratchpad, JsError>(Arc::clone(&sandbox.scratchpad))
    })?;

    scratchpad
        .lock()
        .map_err(|_| JsNativeError::error().with_message("scratchpad mutex poisoned"))?
        .insert(key, value);

    Ok(JsValue::Null)
}

fn recall_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let key = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let scratchpad = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<Scratchpad, JsError>(Arc::clone(&sandbox.scratchpad))
    })?;

    let value = scratchpad
        .lock()
        .map_err(|_| JsNativeError::error().with_message("scratchpad mutex poisoned"))?
        .get(&key)
        .cloned();

    value.map_or(Ok(JsValue::Null), |v| {
        serde_to_jsvalue(&v, ctx).or(Ok(JsValue::Null))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    #[test]
    fn test_registers_remember_and_recall() {
        let mut ctx = Context::default();
        register(&mut ctx).expect("register should not fail");
        for name in ["remember", "recall"] {
            let result = ctx
                .eval(Source::from_bytes(format!("typeof {name}").as_bytes()))
                .unwrap();
            assert_eq!(
                result.as_string().unwrap().to_std_string_escaped(),
                "function",
                "{name} should be a function"
            );
        }
    }

    #[test]
    fn test_new_scratchpad_is_empty() {
        let sp = new_scratchpad();
        assert!(sp.lock().unwrap().is_empty());
    }
}
