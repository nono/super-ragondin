# Web Fetch & Web Search Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `webFetch(url)` and `webSearch(query, options?)` JS sandbox functions to the codemode engine, with `webSearch` gated behind a per-question `--web` flag.

**Architecture:** Two new tool modules following the existing Boa `register()` pattern. `webFetch` uses reqwest directly for HTTP GET. `webSearch` calls Exa via OpenRouter's chat completions API. The `--web` flag flows from CLI/GUI → `CodeModeEngine::ask()` → `Sandbox` → conditional tool registration (same pattern as `askUser`).

**Tech Stack:** Boa JS engine, reqwest, serde_json, wiremock (tests), OpenRouter/Exa API.

---

### Task 1: Add `search_model` to `RagConfig`

**Files:**
- Modify: `crates/rag/src/config.rs`

**Step 1: Write the failing test**

Add to the existing test module in `crates/rag/src/config.rs`:

```rust
#[test]
fn test_config_defaults_include_search_model() {
    temp_env::with_vars_unset(
        [
            "OPENROUTER_API_KEY",
            "OPENROUTER_EMBED_MODEL",
            "OPENROUTER_VISION_MODEL",
            "OPENROUTER_CHAT_MODEL",
            "OPENROUTER_SUBAGENT_MODEL",
            "OPENROUTER_IMAGE_MODEL",
            "OPENROUTER_SEARCH_MODEL",
        ],
        || {
            let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
            assert_eq!(config.search_model, "exa/exa");
        },
    );
}

#[test]
fn test_search_model_from_env() {
    temp_env::with_vars(
        [("OPENROUTER_SEARCH_MODEL", Some("custom/search"))],
        || {
            let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
            assert_eq!(config.search_model, "custom/search");
        },
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-rag config::tests::test_config_defaults_include_search_model -- -q`
Expected: FAIL — `search_model` field doesn't exist.

**Step 3: Write minimal implementation**

Add `search_model: String` field to `RagConfig` struct (after `image_model`). Add it to `Debug` impl. Add to `from_env_with_db_path()`:

```rust
search_model: std::env::var("OPENROUTER_SEARCH_MODEL")
    .unwrap_or_else(|_| "exa/exa".to_string()),
```

Update the existing `test_config_defaults` test to add `"OPENROUTER_SEARCH_MODEL"` to the `with_vars_unset` list.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p super-ragondin-rag -q`
Expected: all tests PASS

**Step 5: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 6: Commit**

```bash
git add crates/rag/src/config.rs
git commit -m "feat(rag): add search_model config field for Exa web search"
```

---

### Task 2: Implement `webFetch` tool

**Files:**
- Create: `crates/codemode/src/tools/web_fetch.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

**Step 1: Write the failing test — registration**

Create `crates/codemode/src/tools/web_fetch.rs` with only the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof webFetch"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
```

Add `pub mod web_fetch;` to `crates/codemode/src/tools/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode web_fetch::tests::test_registers_without_panic -- -q`
Expected: FAIL — `register` not found.

**Step 3: Write `register()` and `web_fetch_fn` implementation**

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::SANDBOX_CTX;

const TIMEOUT_SECS: u64 = 30;
const MAX_BODY_BYTES: usize = 1_048_576; // 1 MB

/// Register the `webFetch(url)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("webFetch"),
        1,
        NativeFunction::from_fn_ptr(web_fetch_fn),
    )?;
    Ok(())
}

fn web_fetch_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let url = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let handle = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<_, JsError>(sandbox.handle.clone())
    })?;

    let result = handle.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .user_agent("SuperRagondin/0.1")
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let is_text = content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("javascript");

        let body = if is_text {
            let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
            let truncated = &bytes[..bytes.len().min(MAX_BODY_BYTES)];
            String::from_utf8_lossy(truncated).into_owned()
        } else {
            String::new()
        };

        Ok::<_, String>(serde_json::json!({
            "status": status,
            "contentType": content_type,
            "body": body,
        }))
    });

    match result {
        Ok(json_val) => crate::sandbox::serde_to_jsvalue(&json_val, ctx),
        Err(e) => Err(JsNativeError::error().with_message(e).into()),
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p super-ragondin-codemode web_fetch -- -q`
Expected: PASS

**Step 5: Write wiremock test for actual fetch behavior**

Add to the same test module:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_web_fetch_returns_status_and_body() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("hello world")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&mock_server)
        .await;

    let (sandbox, _db, _sync) = crate::sandbox::tests::make_sandbox().await;
    let code = format!(r#"webFetch("{}")"#, mock_server.uri());
    let result = sandbox.execute(&code).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], 200);
    assert_eq!(parsed["contentType"], "text/plain");
    assert_eq!(parsed["body"], "hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_web_fetch_binary_returns_empty_body() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0x89, 0x50, 0x4E, 0x47])
                .insert_header("content-type", "image/png"),
        )
        .mount(&mock_server)
        .await;

    let (sandbox, _db, _sync) = crate::sandbox::tests::make_sandbox().await;
    let code = format!(r#"webFetch("{}")"#, mock_server.uri());
    let result = sandbox.execute(&code).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], 200);
    assert_eq!(parsed["body"], "");
}
```

Note: the `make_sandbox()` helper in `sandbox.rs` is `pub(crate)` in the tests module — if it's not accessible, make the `tests` module items `pub(crate)` or duplicate the helper.

**Step 6: Run tests**

Run: `cargo test -p super-ragondin-codemode web_fetch -- -q`
Expected: PASS

**Step 7: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 8: Commit**

```bash
git add crates/codemode/src/tools/web_fetch.rs crates/codemode/src/tools/mod.rs
git commit -m "feat(codemode): add webFetch(url) JS sandbox function"
```

---

### Task 3: Implement `webSearch` tool

**Files:**
- Create: `crates/codemode/src/tools/web_search.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

**Step 1: Write the failing test — registration**

Create `crates/codemode/src/tools/web_search.rs` with the test:

```rust
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
}
```

Add `pub mod web_search;` to `crates/codemode/src/tools/mod.rs`.

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode web_search::tests::test_registers_without_panic -- -q`
Expected: FAIL

**Step 3: Write implementation**

The Exa model on OpenRouter works as a chat completion — send the search query as a user message, receive results as the assistant's content (text with URLs and titles). Parse the response into structured results.

```rust
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

    let messages = vec![
        serde_json::json!({"role": "user", "content": query}),
    ];

    let response_text = handle
        .block_on(async { crate::llm::call_llm(&api_key, &model, messages).await })
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;

    // Parse Exa's response text into structured results.
    // Exa returns markdown-like text with titles, URLs, and snippets.
    let results = parse_exa_response(&response_text, limit);

    serde_to_jsvalue(&serde_json::json!(results), ctx)
}

/// Parse Exa's text response into structured search results.
///
/// Exa returns results as text; this extracts title/url/snippet triples.
/// Falls back to returning the raw text as a single snippet if parsing fails.
fn parse_exa_response(text: &str, limit: u64) -> Vec<serde_json::Value> {
    // Exa's response typically contains markdown links: [Title](URL)\nSnippet text
    // We'll try to extract these, falling back to raw text.
    let mut results = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if results.len() >= limit as usize {
            break;
        }
        let trimmed = line.trim();
        // Look for markdown link pattern: [Title](URL) or numbered items with URLs
        if let Some((title, url)) = extract_markdown_link(trimmed) {
            // Collect subsequent non-link lines as snippet
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

    // Fallback: if no structured results found, return raw text as single result
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
    // Strip optional leading "1. ", "- ", "* ", etc.
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
```

**Step 4: Run tests**

Run: `cargo test -p super-ragondin-codemode web_search -- -q`
Expected: PASS

**Step 5: Add unit tests for `parse_exa_response` and `extract_markdown_link`**

```rust
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
```

**Step 6: Run tests**

Run: `cargo test -p super-ragondin-codemode web_search -- -q`
Expected: all PASS

**Step 7: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 8: Commit**

```bash
git add crates/codemode/src/tools/web_search.rs crates/codemode/src/tools/mod.rs
git commit -m "feat(codemode): add webSearch(query, options?) JS sandbox function"
```

---

### Task 4: Plumb `web_search` flag through Sandbox and Engine

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`
- Modify: `crates/codemode/src/engine.rs`

**Step 1: Write failing test — webSearch not registered by default**

Add to `crates/codemode/src/sandbox.rs` tests:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_web_search_not_registered_by_default() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute("typeof webSearch").unwrap();
    assert_eq!(result, "\"undefined\"", "webSearch must not exist by default");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode sandbox::tests::test_web_search_not_registered_by_default -- -q`
Expected: FAIL (currently `webSearch` is not registered at all, so it might actually pass — but this test documents the intent).

Actually, since `webSearch` isn't registered yet in `run_boa()`, this test passes immediately. Let's first register it unconditionally, see the test fail, then make it conditional.

Instead, write a test that webSearch IS available when `web_search_enabled` is true:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_web_search_registered_when_enabled() {
    use crate::tools::scratchpad::new_scratchpad;
    let db_dir = tempdir().expect("db_dir");
    let sync_dir = tempdir().expect("sync_dir");
    let store = Arc::new(RagStore::open(db_dir.path()).await.expect("store"));
    let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    let sandbox = Sandbox::new(
        store, config,
        sync_dir.path().to_path_buf(),
        new_scratchpad(),
        None, None,
        true, // web_search_enabled
    );
    let result = sandbox.execute("typeof webSearch").unwrap();
    assert_eq!(result, "\"function\"");
}
```

**Step 3: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode sandbox::tests::test_web_search_registered_when_enabled -- -q`
Expected: FAIL — `Sandbox::new()` doesn't take a `web_search_enabled` param yet.

**Step 4: Implement the changes**

In `crates/codemode/src/sandbox.rs`:

1. Add `web_search_enabled: bool` field to `SandboxContext`
2. Add `web_search_enabled: bool` to `Sandbox` struct
3. Add `web_search_enabled: bool` parameter to `Sandbox::new()` (after `interaction`)
4. Pass it through to `SandboxContext` in `execute()`
5. In `run_boa()`, always register `webFetch`, conditionally register `webSearch`:

```rust
tools::web_fetch::register(&mut ctx)
    .map_err(|e| format!("JS error: register webFetch: {e}"))?;
if self.web_search_enabled {
    tools::web_search::register(&mut ctx)
        .map_err(|e| format!("JS error: register webSearch: {e}"))?;
}
```

6. Update `make_sandbox()` test helper to pass `false` for `web_search_enabled`
7. Fix all existing `Sandbox::new()` calls (add `false` as default)

In `crates/codemode/src/engine.rs`:

1. Add `web_search: bool` parameter to `ask()` method (after `context_dir`)
2. Pass `web_search` to `Sandbox::new()` in the `spawn_blocking` closure
3. Update `system_prompt()` call to pass `web_search`

**Step 5: Fix all call sites**

Update `Sandbox::new()` calls in engine.rs test helpers to pass `false`.

**Step 6: Run all tests**

Run: `cargo test -p super-ragondin-codemode -q`
Expected: all PASS

**Step 7: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 8: Commit**

```bash
git add crates/codemode/src/sandbox.rs crates/codemode/src/engine.rs
git commit -m "feat(codemode): plumb web_search flag through sandbox and engine"
```

---

### Task 5: Update system prompt

**Files:**
- Modify: `crates/codemode/src/prompt.rs`
- Modify: `crates/codemode/src/engine.rs` (update `execute_js_tool_definition` description)

**Step 1: Write failing test**

Add to `crates/codemode/src/prompt.rs` tests:

```rust
#[test]
fn test_prompt_contains_web_fetch() {
    let p = system_prompt(false, false);
    assert!(p.contains("webFetch("), "prompt must always mention webFetch");
}

#[test]
fn test_prompt_web_search_included_when_enabled() {
    let p = system_prompt(false, true);
    assert!(p.contains("webSearch("), "web_search prompt must mention webSearch");
}

#[test]
fn test_prompt_web_search_excluded_when_disabled() {
    let p = system_prompt(false, false);
    assert!(!p.contains("webSearch("), "non-web prompt must not mention webSearch");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode prompt::tests -- -q`
Expected: FAIL — `system_prompt` only takes one bool.

**Step 3: Update `system_prompt` signature and content**

Change signature to `pub fn system_prompt(interactive: bool, web_search: bool) -> String`.

Add `webFetch()` docs to the base section (always included):

```
  webFetch(url)
    HTTP GET a URL and return the response.
    Returns: { status: number, contentType: string, body: string }
    Body is text only (empty for binary content types). Max 1 MB, 30s timeout.
```

Add conditional `webSearch()` section (same pattern as `askUser`):

```rust
let web_search_section = if web_search {
    r"  webSearch(query, options?)
    Search the web using Exa. Options: { limit } (default: 5, max: 10)
    Returns: [{ title, url, snippet }, ...]
    Use sparingly — web search has significant API cost.

"
} else {
    ""
};
```

Add an example combining both to the examples section:

```js
// Fetch a web page
const page = webFetch("https://example.com");
subAgent("Summarize this page.", page.body)
```

Update existing tests to use `system_prompt(false, false)` / `system_prompt(true, false)`.

**Step 4: Run tests**

Run: `cargo test -p super-ragondin-codemode prompt::tests -- -q`
Expected: PASS

**Step 5: Update `execute_js_tool_definition()` in engine.rs**

Add `webFetch()` and `webSearch()` to the description string.

**Step 6: Run full crate tests**

Run: `cargo test -p super-ragondin-codemode -q`
Expected: all PASS

**Step 7: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 8: Commit**

```bash
git add crates/codemode/src/prompt.rs crates/codemode/src/engine.rs
git commit -m "feat(codemode): update system prompt with webFetch and webSearch docs"
```

---

### Task 6: Add `--web` flag to CLI

**Files:**
- Modify: `crates/cli/src/main.rs`

**Step 1: Update `cmd_ask` to parse `--web` flag**

In `cmd_ask()` (line 310), before `let question = args.join(" ");`:

```rust
let web_search = args.iter().any(|a| a == "--web");
let question_args: Vec<&str> = args.iter()
    .filter(|a| *a != "--web")
    .map(String::as_str)
    .collect();
if question_args.is_empty() {
    // ... existing suggestion logic (already handles empty args above)
}
let question = question_args.join(" ");
```

Pass `web_search` to `engine.ask(&question, cwd, web_search)`.

**Step 2: Update usage text**

In the help text (line 39), change:
```
  ask <question>                   Ask a question about your files
```
to:
```
  ask [--web] <question>           Ask a question (--web enables web search)
```

**Step 3: Build and verify**

Run: `cargo build -p super-ragondin-cli`
Expected: compiles without errors.

**Step 4: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): add --web flag to ask command for web search"
```

---

### Task 7: Add `web_search` parameter to GUI

**Files:**
- Modify: `crates/gui/src/commands.rs`

**Step 1: Update `ask_question` Tauri command**

Add `web_search: bool` parameter to the `ask_question` function (line 439):

```rust
pub async fn ask_question(
    question: String,
    web_search: bool,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
```

Pass `web_search` to `engine.ask(&question, None, web_search)`.

**Step 2: Update `ask_question_from` test helper** (line 364)

Add `web_search: bool` parameter, pass to `engine.ask()`.

**Step 3: Build and verify**

Run: `cargo build -p super-ragondin-gui`
Expected: compiles. Note: the frontend TypeScript bindings (generated by specta) will need updating too — but that's a frontend change.

**Step 4: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 5: Commit**

```bash
git add crates/gui/src/commands.rs
git commit -m "feat(gui): add web_search parameter to ask_question command"
```

---

### Task 8: Update docs

**Files:**
- Modify: `docs/guides/rag.md`

**Step 1: Update crate structure**

Add to the codemode section:
```
- `src/tools/web_fetch.rs` - `webFetch(url)` JS global — HTTP GET with status/contentType/body response
- `src/tools/web_search.rs` - `webSearch(query, options?)` JS global — Exa web search via OpenRouter (opt-in via `--web` flag)
```

**Step 2: Update environment variables table**

Add:
```
| `OPENROUTER_SEARCH_MODEL` | `exa/exa` | Web search model (Exa via OpenRouter) |
```

**Step 3: Commit**

```bash
git add docs/guides/rag.md
git commit -m "docs: add webFetch and webSearch to rag guide"
```
