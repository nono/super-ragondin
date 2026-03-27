use crate::sandbox::SANDBOX_CTX;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

/// Resolve a raw user input string against a choices list.
///
/// If `input` is a decimal integer in range `1..=choices.len()`, returns the
/// corresponding choice text. Otherwise returns `input` verbatim (trimmed).
#[allow(dead_code)]
pub fn resolve_answer(input: &str, choices: &[&str]) -> String {
    let trimmed = input.trim();
    if let Ok(n) = trimmed.parse::<usize>()
        && n >= 1
        && n <= choices.len()
    {
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
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("askUser"),
        2,
        NativeFunction::from_fn_ptr(ask_user_fn),
    )?;
    Ok(())
}

fn ask_user_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use crate::sandbox::jsvalue_to_serde;
    use boa_engine::JsArgs;

    let question = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_lossy();

    let choices_js = args.get_or_undefined(1);
    let choices_serde = jsvalue_to_serde(choices_js.clone(), ctx);
    let choices_vec: Vec<String> = match &choices_serde {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => {
            return Err(JsNativeError::typ()
                .with_message("askUser: choices must be an array")
                .into());
        }
    };

    if choices_vec.len() < 2 || choices_vec.len() > 3 {
        return Err(JsNativeError::range()
            .with_message(format!(
                "askUser: choices must have 2 or 3 entries, got {}",
                choices_vec.len()
            ))
            .into());
    }

    let interaction = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        sandbox
            .interaction
            .clone()
            .ok_or_else(|| JsNativeError::error().with_message("askUser not available"))
    })?;

    let choices_refs: Vec<&str> = choices_vec.iter().map(String::as_str).collect();
    let answer = interaction.ask(&question, &choices_refs);

    Ok(JsValue::from(js_string!(answer)))
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
