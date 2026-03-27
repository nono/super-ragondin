use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string,
};

/// Resolve a raw user input string against a choices list.
///
/// If `input` is a decimal integer in range `1..=choices.len()`, returns the
/// corresponding choice text. Otherwise returns `input` verbatim (trimmed).
pub fn resolve_answer(input: &str, choices: &[&str]) -> String {
    let trimmed = input.trim();
    if let Ok(n) = trimmed.parse::<usize>() && n >= 1 && n <= choices.len() {
        return choices[n - 1].to_string();
    }
    trimmed.to_string()
}

/// Register the `askUser(question, choices)` global function.
///
/// Only call this when a `UserInteraction` backend is present in `SANDBOX_CTX`.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("askUser"),
        2,
        NativeFunction::from_fn_ptr(ask_user_fn),
    )?;
    Ok(())
}

fn ask_user_fn(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    // Full implementation added in Task 3 when SandboxContext::interaction field exists
    Err(JsNativeError::error()
        .with_message("askUser not available")
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_number_in_range() {
        assert_eq!(resolve_answer("2", &["alpha", "beta", "gamma"]), "beta");
    }

    #[test]
    fn test_resolve_first_choice() {
        assert_eq!(resolve_answer("1", &["yes", "no"]), "yes");
    }

    #[test]
    fn test_resolve_last_choice() {
        assert_eq!(resolve_answer("3", &["x", "y", "z"]), "z");
    }

    #[test]
    fn test_resolve_out_of_range_returns_verbatim() {
        assert_eq!(resolve_answer("0", &["a", "b"]), "0");
        assert_eq!(resolve_answer("5", &["a", "b"]), "5");
    }

    #[test]
    fn test_resolve_freeform_returns_trimmed() {
        assert_eq!(resolve_answer("  my answer  ", &["a", "b"]), "my answer");
    }

    #[test]
    fn test_resolve_nonnumeric_returns_verbatim() {
        assert_eq!(resolve_answer("custom", &["a", "b"]), "custom");
    }
}
