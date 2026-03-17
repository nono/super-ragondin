# Ask Suggestions Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `super-ragondin ask` is invoked with no arguments, generate and print 6 personalized prompt suggestions based on the user's indexed files.

**Architecture:** A new `SuggestionEngine` struct in `crates/codemode/src/suggestions.rs` handles Phase 1 (store queries + parallel summaries with a deadline-based timeout that retains partial results) and Phase 2 (single LLM call with one corrective retry, wrapped in a single 4s timeout). The CLI `cmd_ask` handler detects empty args and delegates to `SuggestionEngine`. The sub-agent HTTP call logic is extracted to a shared `crates/codemode/src/llm.rs` module.

**Tech Stack:** Rust, tokio (async, `timeout`, `spawn`, `Instant`), reqwest (HTTP to OpenRouter), serde_json, `super-ragondin-rag` crate (`RagStore`, `MetadataFilter`, `DocSort`, `DocInfo`, `RagConfig`).

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `crates/codemode/src/llm.rs` | `call_llm(api_key, model, messages) -> Result<String>` — shared HTTP call to OpenRouter |
| Modify | `crates/codemode/src/tools/sub_agent.rs` | Replace local `call_sub_agent` with `crate::llm::call_llm` |
| Create | `crates/codemode/src/suggestions.rs` | `SuggestionEngine` + Phase 1 + Phase 2 logic |
| Modify | `crates/codemode/src/lib.rs` | Export `llm` and `suggestions` modules |
| Modify | `crates/cli/src/main.rs` | Replace `cmd_ask` empty-args path with `SuggestionEngine` |

### Internal structure of `suggestions.rs`

```
query_docs(store, sync_dir, cwd) -> Result<Vec<DocInfo>>     // Phase 1 steps 1-4; pub(crate) for tests
collect_summaries(texts, deadline, summarize_fn) -> Vec<FileContext> // Phase 1 step 5; pub(crate) for tests
generate_suggestions_with_fn(contexts, llm_fn) -> Result<Vec<String>> // Phase 2; pub(crate) for tests
SuggestionEngine::generate(cwd) -> Result<Vec<String>>        // orchestrates both phases
```

Key design decisions:
- `query_docs` returns `Vec<DocInfo>` (pure store queries, fast)
- `collect_summaries` takes pre-prepared `DocText` structs (path + mime + text) — no store dependency — making it fully unit-testable with injected slow/fast closures
- Phase 1 timeout: implemented as a `deadline: tokio::time::Instant` passed into `collect_summaries`, which uses `tokio::time::timeout_at` per task so partial results are retained
- Phase 2 timeout: single `tokio::time::timeout(4s)` wrapping the entire `generate_suggestions_with_fn` call (which internally does initial call + optional retry)

---

## Task 1: Extract shared LLM call into `llm.rs`

**Files:**
- Create: `crates/codemode/src/llm.rs`
- Modify: `crates/codemode/src/tools/sub_agent.rs`
- Modify: `crates/codemode/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/codemode/src/llm.rs` with test only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call_llm_signature_compiles() {
        // Verify the function signature compiles with the expected types.
        // Real HTTP calls require an API key and are integration-tested.
        fn _check_type<'a>(
            api_key: &'a str,
            model: &'a str,
            messages: Vec<serde_json::Value>,
        ) -> impl std::future::Future<Output = anyhow::Result<String>> + 'a {
            call_llm(api_key, model, messages)
        }
        let _ = _check_type;
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /home/nono/dev/super-ragondin/ask-suggestions
cargo test -q -p super-ragondin-codemode 2>&1 | head -20
```

Expected: compile error — `call_llm` not found.

- [ ] **Step 3: Implement `llm.rs`**

```rust
use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

/// Call the OpenRouter chat completions endpoint.
///
/// `messages` is a list of objects with `role` and `content` fields.
///
/// # Errors
/// Returns an error if the HTTP call fails or the response cannot be parsed.
pub async fn call_llm(
    api_key: &str,
    model: &str,
    messages: Vec<serde_json::Value>,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
    });
    let resp = client
        .post(OPENROUTER_API_URL)
        .bearer_auth(api_key)
        .header("HTTP-Referer", OPENROUTER_REFERER)
        .json(&body)
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call_llm_signature_compiles() {
        fn _check_type<'a>(
            api_key: &'a str,
            model: &'a str,
            messages: Vec<serde_json::Value>,
        ) -> impl std::future::Future<Output = anyhow::Result<String>> + 'a {
            call_llm(api_key, model, messages)
        }
        let _ = _check_type;
    }
}
```

- [ ] **Step 4: Add `pub(crate) mod llm;` to `lib.rs`**

In `crates/codemode/src/lib.rs`:
```rust
pub(crate) mod llm;
```

- [ ] **Step 5: Update `sub_agent.rs` to use `crate::llm::call_llm`**

In the `sub_agent_fn` body, replace the `call_sub_agent` call:
```rust
// Before:
sandbox.handle.block_on(async move {
    call_sub_agent(&api_key, &model, &system_prompt, &user_prompt)
        .await
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))
})
// After:
let messages = vec![
    serde_json::json!({"role": "system", "content": system_prompt}),
    serde_json::json!({"role": "user", "content": user_prompt}),
];
sandbox.handle.block_on(async move {
    crate::llm::call_llm(&api_key, &model, messages)
        .await
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))
})
```

Delete the entire `async fn call_sub_agent(...)` function and remove any now-unused `use` imports (`OPENROUTER_API_URL`, `OPENROUTER_REFERER`).

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 7: Run tests**

```bash
cargo test -q -p super-ragondin-codemode
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/codemode/src/llm.rs crates/codemode/src/lib.rs crates/codemode/src/tools/sub_agent.rs
git commit -m "refactor(codemode): extract shared LLM call into llm.rs"
```

---

## Task 2: `query_docs` — Phase 1 store query logic

**Files:**
- Create: `crates/codemode/src/suggestions.rs`
- Modify: `crates/codemode/src/lib.rs`

Implement and test the store-query part of Phase 1 (steps 1–4 of the spec): figure out which docs to use based on `cwd`, with prefix query and fallback.

- [ ] **Step 1: Write the failing tests**

Create `crates/codemode/src/suggestions.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use tempfile::tempdir;
    use super_ragondin_rag::store::RagStore;

    async fn empty_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn test_query_docs_cwd_outside_sync_dir_uses_whole_store() {
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/home/user/other"));
        // strip_prefix fails → whole-store query → empty → Err
        let result = super::query_docs(&store, &sync_dir, cwd).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }

    #[tokio::test]
    async fn test_query_docs_cwd_none_uses_whole_store() {
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let result = super::query_docs(&store, &sync_dir, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }

    #[tokio::test]
    async fn test_query_docs_cwd_inside_sync_dir_prefix_empty_falls_back_to_whole_store() {
        // cwd == sync_dir root → strip_prefix yields "" → treated as no prefix → whole-store
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/tmp/sync"));
        let result = super::query_docs(&store, &sync_dir, cwd).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }

    /// Spec row 1: "cwd inside sync_dir → prefix query used"
    /// Spec row 3: "Prefix query returns 0 → whole-store fallback"
    ///
    /// These require seeding the LanceDB store with real document data, which in turn
    /// requires calling the embedder (needs an API key). They are integration tests.
    /// Skeleton provided here; they will be fleshed out alongside the integration test suite.
    #[tokio::test]
    #[ignore = "requires indexed files (run with --ignored after super-ragondin sync)"]
    async fn test_query_docs_cwd_inside_sync_dir_uses_prefix_query() {
        // Precondition: files must be indexed under /tmp/sync/work/
        // When implemented: assert that results have doc_ids starting with "work/"
        todo!("seed store, then call query_docs with cwd=/tmp/sync/work and assert prefix results")
    }

    #[tokio::test]
    #[ignore = "requires indexed files (run with --ignored after super-ragondin sync)"]
    async fn test_query_docs_prefix_empty_falls_back_to_whole_store_with_results() {
        // Precondition: files indexed, but NOT under the prefix /tmp/sync/emptydir/
        // When implemented: assert that fallback returns whole-store results
        todo!("seed store without data under prefix, verify fallback returns whole-store results")
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode suggestions 2>&1 | head -20
```

Expected: compile error — module not found / `query_docs` not found.

- [ ] **Step 3: Implement `query_docs` and stubs**

Add to `crates/codemode/src/lib.rs`:
```rust
pub mod suggestions;
```

Create `crates/codemode/src/suggestions.rs`:

```rust
use std::path::PathBuf;
use anyhow::{bail, Result};
use super_ragondin_rag::{
    config::RagConfig,
    store::{DocInfo, DocSort, MetadataFilter, RagStore},
};

/// Per-file context collected during Phase 1.
#[derive(Debug, serde::Serialize)]
pub struct FileContext {
    pub path: String,
    pub mime_type: String,
    pub summary: Option<String>,
}

/// Intermediate: path, mime, and pre-fetched text for a document.
struct DocText {
    path: String,
    mime_type: String,
    text: String,
}

pub struct SuggestionEngine {
    config: RagConfig,
    sync_dir: PathBuf,
    store: RagStore,
}

impl SuggestionEngine {
    /// Open the RAG store and construct the engine.
    ///
    /// # Errors
    /// Returns an error if the RAG store cannot be opened.
    pub async fn new(config: RagConfig, sync_dir: PathBuf) -> Result<Self> {
        let store = RagStore::open(&config.db_path).await?;
        Ok(Self { config, sync_dir, store })
    }

    /// Generate 6 prompt suggestions based on recently modified files.
    ///
    /// # Errors
    /// Returns an error if no files are indexed or suggestion generation fails.
    pub async fn generate(&self, cwd: Option<PathBuf>) -> Result<Vec<String>> {
        // Phase 1a: store queries (fast, no timeout)
        let docs = query_docs(&self.store, &self.sync_dir, cwd).await?;

        // Phase 1b: fetch chunk text for up to 5 docs
        let mut doc_texts = Vec::new();
        for doc in docs.iter().take(5) {
            let chunks = self.store.get_chunks(&doc.doc_id).await.unwrap_or_else(|_| vec![]);
            let text: String = chunks.iter().map(|c| c.chunk_text.as_str()).collect::<Vec<_>>().join(" ");
            let text = if text.len() > 2000 { text[..2000].to_string() } else { text };
            doc_texts.push(DocText {
                path: doc.doc_id.clone(),
                mime_type: doc.mime_type.clone(),
                text,
            });
        }
        // Remaining docs beyond 5 get no summary
        let rest: Vec<FileContext> = docs[doc_texts.len()..].iter().map(|d| FileContext {
            path: d.doc_id.clone(),
            mime_type: d.mime_type.clone(),
            summary: None,
        }).collect();

        // Phase 1c: parallel summaries with 3s deadline
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        let api_key = self.config.api_key.clone();
        let model = self.config.subagent_model.clone();
        let mut contexts = collect_summaries(doc_texts, deadline, move |text: String| {
            let api_key = api_key.clone();
            let model = model.clone();
            async move { summarize(&api_key, &model, &text).await }
        }).await;
        contexts.extend(rest);

        // Phase 2: generate suggestions with single 4s timeout (covers initial call + retry)
        let api_key = self.config.api_key.clone();
        let model = self.config.subagent_model.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(4),
            generate_suggestions_with_fn(contexts, move |messages| {
                let api_key = api_key.clone();
                let model = model.clone();
                async move { crate::llm::call_llm(&api_key, &model, messages).await }
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("suggestion generation timed out"))?
    }
}

/// Phase 1, steps 1–4: determine which docs to use.
/// Returns up to 10 DocInfo sorted by most-recently-modified.
///
/// # Errors
/// Returns `"no files indexed"` error if the store is empty.
pub(crate) async fn query_docs(
    store: &RagStore,
    sync_dir: &std::path::Path,
    cwd: Option<PathBuf>,
) -> Result<Vec<DocInfo>> {
    // Step 1-2: try prefix query if cwd is inside sync_dir and prefix is non-empty
    let prefix = cwd.as_deref().and_then(|c| c.strip_prefix(sync_dir).ok()).and_then(|rel| {
        let s = rel.to_string_lossy().into_owned();
        if s.is_empty() { None } else { Some(s) }
    });

    let docs = if let Some(prefix) = prefix {
        let filter = MetadataFilter {
            path_prefix: Some(prefix),
            mime_type: None,
            after: None,
            before: None,
        };
        let docs = store.list_docs(Some(&filter), DocSort::Recent, Some(10)).await?;
        if docs.is_empty() {
            // Step 3: prefix returned nothing — fallback to whole-store
            store.list_docs(None, DocSort::Recent, Some(10)).await?
        } else {
            docs
        }
    } else {
        // Step 3: no usable prefix — whole-store query
        store.list_docs(None, DocSort::Recent, Some(10)).await?
    };

    // Step 4: empty store
    if docs.is_empty() {
        bail!("no files indexed");
    }
    Ok(docs)
}

/// Phase 1, step 5: run summaries in parallel, retaining partial results if deadline passes.
///
/// `summarize_fn` is injected for testability (unit tests pass fast/slow closures).
pub(crate) async fn collect_summaries<F, Fut>(
    doc_texts: Vec<DocText>,
    deadline: tokio::time::Instant,
    summarize_fn: F,
) -> Vec<FileContext>
where
    F: Fn(String) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let mut handles: Vec<(String, String, tokio::task::JoinHandle<Result<String>>)> = Vec::new();
    for dt in doc_texts {
        let fn_clone = summarize_fn.clone();
        let text = dt.text.clone();
        let handle = tokio::spawn(async move { fn_clone(text).await });
        handles.push((dt.path, dt.mime_type, handle));
    }

    let mut contexts = Vec::new();
    for (path, mime_type, handle) in handles {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let summary = if remaining.is_zero() {
            handle.abort();
            None
        } else {
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(Ok(Ok(s))) => Some(s),
                _ => None,
            }
        };
        contexts.push(FileContext { path, mime_type, summary });
    }
    contexts
}

async fn summarize(api_key: &str, model: &str, text: &str) -> Result<String> {
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are a helpful assistant. Summarize the following document in one sentence of at most 20 words."}),
        serde_json::json!({"role": "user", "content": text}),
    ];
    crate::llm::call_llm(api_key, model, messages).await
}

const PHASE2_SYSTEM_PROMPT: &str = "\
You are a creative assistant helping a user discover what they can ask their personal document AI.\n\
\n\
Given the following list of recently modified files (with optional summaries), generate exactly 6 prompt suggestions.\n\
- 2 suggestions must be practical (obvious, immediately useful)\n\
- 4 suggestions must be creative, surprising, or delightful — things the user would not think to ask on their own\n\
- Each suggestion must be specific to the actual content provided, not generic\n\
- Each suggestion must be under 80 characters\n\
- Return ONLY a JSON array of 6 strings, with no other text or explanation";

/// Phase 2: generate suggestions via LLM with one corrective retry on bad JSON.
///
/// The caller is responsible for wrapping this in a timeout.
/// `llm_fn` is injected for testability.
pub(crate) async fn generate_suggestions_with_fn<F, Fut>(
    contexts: Vec<FileContext>,
    llm_fn: F,
) -> Result<Vec<String>>
where
    F: Fn(Vec<serde_json::Value>) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let user_prompt = serde_json::to_string(&contexts)?;
    let mut messages = vec![
        serde_json::json!({"role": "system", "content": PHASE2_SYSTEM_PROMPT}),
        serde_json::json!({"role": "user", "content": user_prompt}),
    ];

    let response = llm_fn(messages.clone()).await?;
    if let Ok(suggestions) = parse_suggestions(&response) {
        return Ok(suggestions);
    }

    // Corrective retry (same timeout window — managed by caller)
    messages.push(serde_json::json!({"role": "assistant", "content": response}));
    messages.push(serde_json::json!({
        "role": "user",
        "content": "Your response was not valid JSON. Return ONLY a JSON array of 6 strings."
    }));
    let response2 = llm_fn(messages).await?;
    parse_suggestions(&response2)
        .map_err(|_| anyhow::anyhow!("failed to parse suggestions after retry"))
}

fn parse_suggestions(text: &str) -> Result<Vec<String>> {
    let text = text.trim();
    // Strip optional markdown code fence
    let text = text.strip_prefix("```json").unwrap_or(text);
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text).trim();

    let suggestions: Vec<String> = serde_json::from_str(text)?;
    if suggestions.len() != 6 {
        bail!("expected 6 suggestions, got {}", suggestions.len());
    }
    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use tempfile::tempdir;
    use super_ragondin_rag::store::RagStore;

    async fn empty_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn test_query_docs_cwd_outside_sync_dir_uses_whole_store() {
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/home/user/other"));
        let result = super::query_docs(&store, &sync_dir, cwd).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }

    #[tokio::test]
    async fn test_query_docs_cwd_none_uses_whole_store() {
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let result = super::query_docs(&store, &sync_dir, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }

    #[tokio::test]
    async fn test_query_docs_cwd_at_sync_dir_root_uses_whole_store() {
        // cwd == sync_dir → strip_prefix yields "" → no prefix → whole-store
        let (store, _dir) = empty_store().await;
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/tmp/sync"));
        let result = super::query_docs(&store, &sync_dir, cwd).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no files indexed"));
    }
}
```

- [ ] **Step 4: Add `tempfile` dev-dependency**

```bash
cargo add --dev tempfile -p super-ragondin-codemode
```

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 6: Run tests**

```bash
cargo test -q -p super-ragondin-codemode suggestions 2>&1
```

Expected: 3 new tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/codemode/src/suggestions.rs crates/codemode/src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(suggestions): add SuggestionEngine skeleton with Phase 1 query logic"
```

---

## Task 3: Phase 1 partial-timeout test + Phase 2 tests

**Files:**
- Modify: `crates/codemode/src/suggestions.rs`

Add the remaining unit tests: partial-timeout behavior for `collect_summaries`, and Phase 2 JSON parse/retry tests for `generate_suggestions_with_fn`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module inside `suggestions.rs`:

```rust
    #[tokio::test]
    async fn test_collect_summaries_partial_timeout_retains_completed() {
        use std::time::Duration;
        use super::{DocText, collect_summaries};

        let doc_texts = vec![
            DocText { path: "fast.md".to_string(), mime_type: "text/plain".to_string(), text: "fast".to_string() },
            DocText { path: "slow.md".to_string(), mime_type: "text/plain".to_string(), text: "slow".to_string() },
        ];
        // Deadline: 100ms — fast doc completes immediately, slow doc takes 500ms
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        let contexts = collect_summaries(doc_texts, deadline, |text: String| async move {
            if text == "slow" {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(format!("summary of {text}"))
        }).await;

        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].summary.as_deref(), Some("summary of fast"));
        assert!(contexts[1].summary.is_none(), "slow doc should have no summary");
    }

    #[tokio::test]
    async fn test_phase2_bad_json_then_corrective_retry_succeeds() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        use super::generate_suggestions_with_fn;

        let count = Arc::new(AtomicUsize::new(0));
        let count2 = count.clone();
        let result = generate_suggestions_with_fn(vec![], move |_messages| {
            let n = count2.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Ok("not valid json".to_string())
                } else {
                    Ok(r#"["a","b","c","d","e","f"]"#.to_string())
                }
            }
        })
        .await;

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap().len(), 6);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_phase2_bad_json_twice_returns_err() {
        use super::generate_suggestions_with_fn;
        let result = generate_suggestions_with_fn(vec![], |_| async { Ok("not valid json".to_string()) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_phase2_wrong_length_returns_err() {
        use super::generate_suggestions_with_fn;
        let result = generate_suggestions_with_fn(vec![], |_| async { Ok(r#"["a","b","c"]"#.to_string()) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_phase2_markdown_fenced_json_parses_ok() {
        use super::generate_suggestions_with_fn;
        let json = "```json\n[\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]\n```";
        let result = generate_suggestions_with_fn(vec![], |_| async move { Ok(json.to_string()) }).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }
```

Note: `DocText` is currently private. To test `collect_summaries` directly, make `DocText` and `collect_summaries` `pub(crate)`.

- [ ] **Step 2: Make `DocText` and `collect_summaries` `pub(crate)`**

In `suggestions.rs`:
- Change `struct DocText` → `pub(crate) struct DocText`
- `collect_summaries` is already `pub(crate)` from Task 2

- [ ] **Step 3: Run tests to see them fail**

```bash
cargo test -q -p super-ragondin-codemode suggestions 2>&1
```

Expected: compile error (test module references `DocText`) then tests fail ("not yet implemented" stubs are gone — tests should now just run and fail on logic).

- [ ] **Step 4: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error"
```

- [ ] **Step 5: Run all suggestions tests**

```bash
cargo test -q -p super-ragondin-codemode suggestions 2>&1
```

Expected: all 8 tests pass (3 from Task 2 + 5 new).

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/suggestions.rs
git commit -m "test(suggestions): add partial-timeout and Phase 2 retry unit tests"
```

---

## Task 4: CLI integration

**Files:**
- Modify: `crates/cli/src/main.rs`

Replace the current empty-args path in `cmd_ask` (which prints a usage message) with the `SuggestionEngine` flow.

- [ ] **Step 1: Locate the empty-args guard**

Find the block that currently reads:
```rust
if args.is_empty() {
    println!("Usage: super-ragondin ask <question>");
    return Ok(());
}
```

It is near the top of `cmd_ask` in `crates/cli/src/main.rs`.

- [ ] **Step 2: Replace the empty-args block**

Replace that block with:

```rust
if args.is_empty() {
    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;
    let db_path = config.rag_dir();
    let rag_config = RagConfig::from_env_with_db_path(db_path);
    if rag_config.api_key.is_empty() {
        return Err(Error::Permanent(
            "OPENROUTER_API_KEY environment variable not set".to_string(),
        ));
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let engine =
            super_ragondin_codemode::suggestions::SuggestionEngine::new(rag_config, config.sync_dir)
                .await
                .map_err(|e| Error::Permanent(format!("{e:#}")))?;
        let cwd = std::env::current_dir().ok();
        match engine.generate(cwd).await {
            Ok(suggestions) => {
                println!("Not sure what to ask? Here are some ideas:\n");
                for (i, s) in suggestions.iter().enumerate() {
                    println!("{}. {}", i + 1, s);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("no files indexed") {
                    println!("No files indexed yet. Run super-ragondin sync first.");
                } else {
                    println!(
                        "Could not generate suggestions. Try: super-ragondin ask <your question>"
                    );
                }
            }
        }
        Ok(())
    })?;
    return Ok(());
}
```

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 4: Build to verify it compiles**

```bash
cargo build -p super-ragondin 2>&1
```

Expected: compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): show suggestions when ask is invoked with no arguments"
```

---

## Task 5: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run all tests**

```bash
cargo test -q 2>&1
```

Expected: all pass, no regressions.

- [ ] **Step 2: Full lint check**

```bash
cargo clippy --all-features 2>&1 | grep -E "^error|warning\[" | head -20
```

Expected: no errors, no new warnings.

- [ ] **Step 3: Smoke test (if API key and indexed files are available)**

```bash
super-ragondin ask
```

Expected (with files indexed):
```
Not sure what to ask? Here are some ideas:

1. <suggestion>
2. <suggestion>
3. <suggestion>
4. <suggestion>
5. <suggestion>
6. <suggestion>
```

Expected (no files indexed):
```
No files indexed yet. Run super-ragondin sync first.
```
