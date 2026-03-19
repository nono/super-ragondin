use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_cozy_client::types::MailPart;

use crate::sandbox::SANDBOX_CTX;

/// Register the `sendMail(subject, body)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("sendMail"),
        0,
        NativeFunction::from_fn_ptr(send_mail_fn),
    )?;
    Ok(())
}

fn send_mail_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let subject = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let body = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_lossy();

    let (cozy_client, handle) = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let client = sandbox
            .cozy_client
            .clone()
            .ok_or_else(|| JsNativeError::error().with_message("Cozy client not configured"))?;
        Ok::<_, JsError>((client, sandbox.handle.clone()))
    })?;

    let parts = [MailPart::plain(body)];

    handle
        .block_on(cozy_client.send_mail(&subject, &parts))
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
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof sendMail"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
