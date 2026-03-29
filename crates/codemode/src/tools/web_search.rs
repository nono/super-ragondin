use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::{SANDBOX_CTX, jsvalue_to_serde, serde_to_jsvalue};

const DEFAULT_LIMIT: u64 = 5;
const MAX_LIMIT: u64 = 10;

/// Register the `webSearch(query, options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("webSearch"),
        1,
        NativeFunction::from_fn_ptr(web_search_fn),
    )?;
    Ok(())
}

fn web_search_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let query = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let options = if args.len() > 1 && !args[1].is_undefined() {
        jsvalue_to_serde(args[1].clone(), ctx)
    } else {
        serde_json::Value::Null
    };

    let limit = options
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);

    let (api_key, model, handle) = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<_, JsError>((
            sandbox.config.api_key.clone(),
            sandbox.config.search_model.clone(),
            sandbox.handle.clone(),
        ))
    })?;

    let messages = vec![serde_json::json!({"role": "user", "content": query})];

    let response_text = handle
        .block_on(async { crate::llm::call_llm(&api_key, &model, messages).await })
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;

    let results = parse_exa_response(&response_text, limit);

    serde_to_jsvalue(&serde_json::json!(results), ctx)
}

/// Parse Exa's text response into structured search results.
///
/// Exa returns results as text with markdown links. This extracts title/url/snippet triples.
/// Falls back to returning the raw text as a single snippet if parsing fails.
fn parse_exa_response(text: &str, limit: u64) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if results.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
            break;
        }
        let trimmed = line.trim();
        if let Some((title, url)) = extract_markdown_link(trimmed) {
            let mut snippet_lines = Vec::new();
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim();
                if next_trimmed.is_empty() || extract_markdown_link(next_trimmed).is_some() {
                    break;
                }
                snippet_lines.push(next_trimmed);
                lines.next();
            }
            results.push(serde_json::json!({
                "title": title,
                "url": url,
                "snippet": snippet_lines.join(" "),
            }));
        }
    }

    if results.is_empty() {
        results.push(serde_json::json!({
            "title": "",
            "url": "",
            "snippet": text,
        }));
    }

    results
}

/// Extract a markdown link `[title](url)` from a line.
fn extract_markdown_link(line: &str) -> Option<(String, String)> {
    let stripped = line
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '*')
        .trim_start();
    if !stripped.starts_with('[') {
        return None;
    }
    let close_bracket = stripped.find(']')?;
    let title = stripped[1..close_bracket].to_string();
    let rest = &stripped[close_bracket + 1..];
    if !rest.starts_with('(') {
        return None;
    }
    let close_paren = rest.find(')')?;
    let url = rest[1..close_paren].to_string();
    if url.starts_with("http") {
        Some((title, url))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof webSearch"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_extract_markdown_link() {
        let (title, url) = extract_markdown_link("[Rust](https://www.rust-lang.org)").unwrap();
        assert_eq!(title, "Rust");
        assert_eq!(url, "https://www.rust-lang.org");
    }

    #[test]
    fn test_extract_markdown_link_with_list_prefix() {
        let (title, url) = extract_markdown_link("1. [Docs](https://docs.rs)").unwrap();
        assert_eq!(title, "Docs");
        assert_eq!(url, "https://docs.rs");
    }

    #[test]
    fn test_extract_markdown_link_no_match() {
        assert!(extract_markdown_link("plain text").is_none());
    }

    #[test]
    fn test_parse_exa_response_structured() {
        let text = "[Result One](https://example.com/1)\nA snippet about result one.\n\n[Result Two](https://example.com/2)\nAnother snippet.";
        let results = parse_exa_response(text, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"], "Result One");
        assert_eq!(results[0]["url"], "https://example.com/1");
        assert_eq!(results[0]["snippet"], "A snippet about result one.");
        assert_eq!(results[1]["title"], "Result Two");
    }

    #[test]
    fn test_parse_exa_response_respects_limit() {
        let text = "[A](https://a.com)\nSnip A\n\n[B](https://b.com)\nSnip B\n\n[C](https://c.com)\nSnip C";
        let results = parse_exa_response(text, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_parse_exa_response_fallback() {
        let text = "No structured results here, just plain text.";
        let results = parse_exa_response(text, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["snippet"], text);
    }
}
