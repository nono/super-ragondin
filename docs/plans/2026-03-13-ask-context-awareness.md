# Ask Context Awareness Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inject a context message (current directory + recently modified files) into the LLM conversation when the user runs `ask`, improving retrieval relevance and output placement.

**Architecture:** `cmd_ask` passes `std::env::current_dir().ok()` to `CodeModeEngine::ask(question, context_dir)`. The engine strips the sync_dir prefix to get a relative path, queries the RagStore for files modified in the last 15 minutes via a new `list_recent` method, assembles a `[Context]` user message, and prepends it to the conversation before the actual question.

**Tech Stack:** Rust, LanceDB (`lancedb`), `std::time::SystemTime`, Boa JS engine (unchanged), OpenRouter API (unchanged).

---

## Chunk 1: Add `RagStore::list_recent`

**Files:**
- Modify: `crates/rag/src/store.rs`

### Task 1: Add `list_recent` to `RagStore`

**Files:**
- Modify: `crates/rag/src/store.rs`

- [ ] **Step 1: Write the failing test**

Add this test at the bottom of `crates/rag/src/store.rs`, inside the existing `#[cfg(test)]` module (check for one at the end of the file; if there is none, add a new `#[cfg(test)] mod tests { ... }` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn unix_secs(t: SystemTime) -> i64 {
        t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
    }

    async fn make_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RagStore::open(dir.path()).await.expect("open");
        (store, dir)
    }

    fn dummy_chunk(doc_id: &str, mtime: SystemTime) -> ChunkRecord {
        ChunkRecord {
            id: format!("{doc_id}-0"),
            doc_id: doc_id.to_string(),
            mime_type: "text/plain".to_string(),
            mtime: unix_secs(mtime),
            chunk_index: 0,
            chunk_text: "hello".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 3072],
        }
    }

    #[tokio::test]
    async fn test_list_recent_returns_only_recent_docs() {
        let (store, _dir) = make_store().await;
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(60);   // 1 min ago
        let old    = now - Duration::from_secs(3600); // 1 hour ago

        store.upsert_chunks(&[dummy_chunk("docs/new.md", recent)]).await.unwrap();
        store.upsert_chunks(&[dummy_chunk("docs/old.md", old)]).await.unwrap();

        let since = now - Duration::from_secs(900); // 15 min window
        let result = store.list_recent(since).await.unwrap();

        assert_eq!(result, vec!["docs/new.md".to_string()]);
    }

    #[tokio::test]
    async fn test_list_recent_empty_when_nothing_recent() {
        let (store, _dir) = make_store().await;
        let now = SystemTime::now();
        let old = now - Duration::from_secs(3600);

        store.upsert_chunks(&[dummy_chunk("docs/old.md", old)]).await.unwrap();

        let since = now - Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_list_recent_deduplicates_doc_ids() {
        let (store, _dir) = make_store().await;
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(60);

        // Two chunks for same doc
        let mut c0 = dummy_chunk("docs/multi.md", recent);
        let mut c1 = dummy_chunk("docs/multi.md", recent);
        c0.id = "docs/multi.md-0".to_string();
        c0.chunk_index = 0;
        c1.id = "docs/multi.md-1".to_string();
        c1.chunk_index = 1;
        store.upsert_chunks(&[c0, c1]).await.unwrap();

        let since = now - Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "docs/multi.md");
    }

    #[tokio::test]
    async fn test_list_recent_caps_at_20() {
        let (store, _dir) = make_store().await;
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(60);

        let chunks: Vec<ChunkRecord> = (0..25_u32)
            .map(|i| dummy_chunk(&format!("docs/file{i}.md"), recent))
            .collect();
        store.upsert_chunks(&chunks).await.unwrap();

        let since = now - Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert!(result.len() <= 20, "got {} results, expected <= 20", result.len());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -q --package super-ragondin-rag list_recent 2>&1 | tail -20
```

Expected: compilation error — `list_recent` method not found.

- [ ] **Step 3: Implement `list_recent`**

In `crates/rag/src/store.rs`, add the following method to the `impl RagStore` block (after `list_docs`):

```rust
/// Return the `doc_id`s of documents modified after `since`, most recent first.
///
/// Uses the existing [`MetadataFilter`] `after` field (Unix timestamp seconds).
/// De-duplicates across chunks and caps results at 20.
///
/// # Errors
/// Returns error if the database query fails.
pub async fn list_recent(&self, since: std::time::SystemTime) -> Result<Vec<String>> {
    let since_secs = since
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let filter = MetadataFilter {
        mime_type: None,
        path_prefix: None,
        after: Some(since_secs),
        before: None,
    };
    let docs = self
        .list_docs(Some(&filter), DocSort::Recent, Some(20))
        .await?;
    Ok(docs.into_iter().map(|d| d.doc_id).collect())
}
```

- [ ] **Step 4: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features --package super-ragondin-rag 2>&1 | tail -20
```

Expected: no warnings.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -q --package super-ragondin-rag list_recent 2>&1 | tail -20
```

Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/store.rs
git commit -m "feat(rag): add RagStore::list_recent for 15-min context window"
```

---

## Chunk 2: Context message assembly in `CodeModeEngine`

**Files:**
- Modify: `crates/codemode/src/engine.rs`

### Task 2: Add `context_dir` parameter and `build_context_message` to `CodeModeEngine`

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)]` mod in `crates/codemode/src/engine.rs`:

```rust
#[cfg(test)]
mod tests {
    // ... keep existing tests, add below:

    // Helpers for build_context_message tests
    fn make_engine_for_ctx_test() -> (CodeModeEngine, tempfile::TempDir, tempfile::TempDir) {
        // We need a sync CodeModeEngine — construct one synchronously using
        // a pre-opened store. Use tokio::runtime for the async open.
        use std::sync::Arc;
        use super_ragondin_rag::{config::RagConfig, store::RagStore};
        let db_dir = tempfile::tempdir().expect("db_dir");
        let sync_dir = tempfile::tempdir().expect("sync_dir");
        let store = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(RagStore::open(db_dir.path()))
            .expect("store");
        let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
        let engine = CodeModeEngine {
            store: Arc::new(store),
            config,
            sync_dir: sync_dir.path().to_path_buf(),
        };
        (engine, db_dir, sync_dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_none_when_no_signals() {
        let (engine, _db, _sync) = make_engine_for_ctx_test();
        // context_dir outside sync_dir, no recent files
        let result = engine.build_context_message(None).await;
        assert!(result.is_none(), "should be None when no signals");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_includes_relative_dir() {
        let (engine, _db, sync) = make_engine_for_ctx_test();
        // context_dir is a subdirectory inside sync_dir
        let sub = sync.path().join("work/meetings");
        std::fs::create_dir_all(&sub).unwrap();
        let result = engine.build_context_message(Some(sub)).await;
        let msg = result.expect("should have message");
        assert!(msg.contains("Current directory:"), "got: {msg}");
        assert!(msg.contains("work/meetings"), "got: {msg}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_outside_sync_dir_no_cwd_line() {
        let (engine, _db, _sync) = make_engine_for_ctx_test();
        // context_dir is outside sync_dir
        let outside = tempfile::tempdir().unwrap();
        // No recent files either, so result is None
        let result = engine.build_context_message(Some(outside.path().to_path_buf())).await;
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_context_message_recent_files_no_cwd() {
        // (no CWD, some recent files) — should still return Some with recent list
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        use super_ragondin_rag::store::ChunkRecord;
        let (engine, _db, _sync) = make_engine_for_ctx_test();
        let now = SystemTime::now();
        let recent_secs = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - 60; // 1 min ago
        let chunk = ChunkRecord {
            id: "docs/recent.md-0".to_string(),
            doc_id: "docs/recent.md".to_string(),
            mime_type: "text/plain".to_string(),
            mtime: recent_secs,
            chunk_index: 0,
            chunk_text: "hello".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 3072],
        };
        engine.store.upsert_chunks(&[chunk]).await.unwrap();

        let result = engine.build_context_message(None).await;
        let msg = result.expect("should have message when recent files exist");
        assert!(msg.contains("Recently modified"), "got: {msg}");
        assert!(msg.contains("docs/recent.md"), "got: {msg}");
        assert!(!msg.contains("Current directory:"), "should not have CWD line; got: {msg}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_context_message_prepended_before_question() {
        // Verify that when build_context_message returns Some, the message list
        // has it at index 1 (after system prompt) and the question at index 2.
        // We can't call ask() end-to-end without an API key, so we test the
        // message assembly logic via build_context_message directly.
        let (engine, _db, sync) = make_engine_for_ctx_test();
        let sub = sync.path().join("notes");
        std::fs::create_dir_all(&sub).unwrap();
        let ctx_msg = engine.build_context_message(Some(sub)).await;
        assert!(ctx_msg.is_some());
        let msg = ctx_msg.unwrap();
        assert!(msg.starts_with("[Context]"), "got: {msg}");
    }
}
```

- [ ] **Step 2: Run to verify tests fail**

```bash
cargo test -q --package super-ragondin-codemode build_context_message 2>&1 | tail -20
```

Expected: compilation error — `build_context_message` not found, `context_dir` param missing.

- [ ] **Step 3: Update `ask` signature and add `build_context_message`**

In `crates/codemode/src/engine.rs`:

**3a.** Change `ask` signature from:
```rust
pub async fn ask(&self, question: &str) -> Result<()> {
```
to:
```rust
pub async fn ask(&self, question: &str, context_dir: Option<std::path::PathBuf>) -> Result<()> {
```

**3b.** At the start of `ask`, before building `messages`, insert the context message assembly:
```rust
let context_msg = self.build_context_message(context_dir).await;

let mut messages = vec![
    serde_json::json!({"role": "system", "content": system_prompt()}),
];
if let Some(ctx) = context_msg {
    messages.push(serde_json::json!({"role": "user", "content": ctx}));
}
messages.push(serde_json::json!({"role": "user", "content": question}));
```

**3c.** Add the private method after `ask`:
```rust
/// Build the optional context message to prepend before the user question.
///
/// Returns `None` if there are no signals to report (no CWD inside sync_dir
/// and no recently modified files).
async fn build_context_message(
    &self,
    context_dir: Option<std::path::PathBuf>,
) -> Option<String> {
    // Compute relative CWD if inside sync_dir
    let relative_cwd: Option<String> = context_dir.and_then(|dir| {
        dir.strip_prefix(&self.sync_dir)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    });

    // Query recent files (last 15 minutes)
    let since = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(900))
        .unwrap_or(std::time::UNIX_EPOCH);
    let recent_files = self.store.list_recent(since).await.unwrap_or_default();

    if relative_cwd.is_none() && recent_files.is_empty() {
        return None;
    }

    let mut lines = vec!["[Context]".to_string()];
    if let Some(ref cwd) = relative_cwd {
        let display = if cwd.is_empty() { "." } else { cwd.as_str() };
        lines.push(format!("Current directory: {display}"));
    }
    if !recent_files.is_empty() {
        lines.push("Recently modified (last 15 min):".to_string());
        for path in &recent_files {
            lines.push(format!("- {path}"));
        }
    }
    Some(lines.join("\n"))
}
```

- [ ] **Step 4: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features --package super-ragondin-codemode 2>&1 | tail -20
```

Expected: no warnings.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -q --package super-ragondin-codemode 2>&1 | tail -20
```

Expected: all tests pass (including pre-existing ones; note existing `ask`-related tests may need `None` added as second argument — fix those now if they fail).

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/engine.rs
git commit -m "feat(codemode): inject context_dir and recent files into ask conversation"
```

---

## Chunk 3: Wire `context_dir` in the CLI

**Files:**
- Modify: `crates/cli/src/main.rs`

### Task 3: Pass CWD to `engine.ask()` from `cmd_ask`

- [ ] **Step 1: Update `cmd_ask` to pass CWD**

In `crates/cli/src/main.rs`, find `cmd_ask`. Change the `engine.ask(&question)` call:

```rust
// Before:
engine
    .ask(&question)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;

// After:
let cwd = std::env::current_dir().ok();
engine
    .ask(&question, cwd)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
```

- [ ] **Step 2: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | tail -20
```

Expected: no warnings.

- [ ] **Step 3: Build the full workspace**

```bash
cargo build 2>&1 | tail -20
```

Expected: builds cleanly.

- [ ] **Step 4: Run all tests**

```bash
cargo test -q 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): pass current working directory to ask for context injection"
```
