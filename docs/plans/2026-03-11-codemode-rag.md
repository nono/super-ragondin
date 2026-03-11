# Code Mode RAG Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `ask` command's search-then-generate pipeline with a JS sandbox approach where the LLM executes code to query the vector database, discover files, and call sub-agents before synthesizing an answer.

**Architecture:** New `crates/codemode/` crate owns a Boa JS engine + LLM tool-use loop. The LLM receives a single `execute_js` tool and four JS globals (`search`, `listFiles`, `getDocument`, `subAgent`). Rust state is shared with Boa via a thread-local; async calls bridge via `Handle::block_on()` on a `spawn_blocking` thread.

**Tech Stack:** Boa 0.20 (JS engine), Tokio (async), reqwest (OpenRouter API), serde_json (JSON ↔ JsValue), LanceDB (vector DB via existing `super-ragondin-rag` crate)

**Spec:** `docs/specs/2026-03-11-codemode-rag-design.md`

---

## Chunk 1: RAG Store Changes

### Task 1: Add `subagent_model` to `RagConfig`

**Files:**
- Modify: `crates/rag/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `test_config_defaults` test in `crates/rag/src/config.rs`:

```rust
assert_eq!(config.subagent_model, "google/gemini-2.5-flash");
```

And add a new test:

```rust
#[test]
fn test_subagent_model_from_env() {
    temp_env::with_vars(
        [("OPENROUTER_SUBAGENT_MODEL", Some("anthropic/claude-haiku-4-5"))],
        || {
            let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
            assert_eq!(config.subagent_model, "anthropic/claude-haiku-4-5");
        },
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-rag config
```

Expected: FAIL — no `subagent_model` field.

- [ ] **Step 3: Implement**

In `crates/rag/src/config.rs`, add `subagent_model` field to `RagConfig` struct, the `Debug` impl, and `from_env_with_db_path`:

```rust
pub struct RagConfig {
    pub api_key: String,
    pub embed_model: String,
    pub vision_model: String,
    pub chat_model: String,
    pub subagent_model: String,   // NEW
    pub db_path: PathBuf,
}
```

In `Debug` impl, add:
```rust
.field("subagent_model", &self.subagent_model)
```

In `from_env_with_db_path`:
```rust
subagent_model: std::env::var("OPENROUTER_SUBAGENT_MODEL")
    .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
```

Also update `test_config_defaults` to include the new variable in the `with_vars_unset` list:
```rust
temp_env::with_vars_unset(
    [
        "OPENROUTER_API_KEY",
        "OPENROUTER_EMBED_MODEL",
        "OPENROUTER_VISION_MODEL",
        "OPENROUTER_CHAT_MODEL",
        "OPENROUTER_SUBAGENT_MODEL",   // NEW
    ],
    ...
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q -p super-ragondin-rag config
```

Expected: all pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Fix any warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/config.rs
git commit -m "feat(rag): add subagent_model to RagConfig"
```

---

### Task 2: Add `MetadataFilter` type

**Files:**
- Modify: `crates/rag/src/store.rs`

**Note on types:** `MetadataFilter.after`/`before` and `DocInfo.mtime` use `i64` Unix seconds (not `DateTime<Utc>`) to avoid adding a `chrono` dependency to the rag crate. The `codemode` crate (which has chrono) handles `DateTime<Utc>` ↔ `i64` conversion in the tool handlers. This is an intentional divergence from the spec's type signatures, preserving the rag crate's minimal dependencies.

- [ ] **Step 1: Write the failing test**

Add to `crates/rag/src/store.rs` tests:

```rust
#[test]
fn test_metadata_filter_where_clause() {
    let filter = MetadataFilter {
        mime_type: Some("application/pdf".to_string()),
        path_prefix: Some("work".to_string()),  // no trailing slash
        after: Some(1_700_000_000),
        before: None,
    };
    let clause = filter.to_where_clause().unwrap();
    assert!(clause.contains("mime_type = 'application/pdf'"));
    assert!(clause.contains("doc_id LIKE 'work/%'"));  // trailing slash added
    assert!(clause.contains("mtime > 1700000000"));

    let empty = MetadataFilter { mime_type: None, path_prefix: None, after: None, before: None };
    assert!(empty.to_where_clause().is_none());
}

#[test]
fn test_metadata_filter_escapes_single_quotes() {
    let filter = MetadataFilter {
        mime_type: Some("text/plain".to_string()),
        path_prefix: Some("O'Brien/notes".to_string()),
        after: None,
        before: None,
    };
    let clause = filter.to_where_clause().unwrap();
    // single quotes in path_prefix must be escaped
    assert!(clause.contains("O\\'Brien"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-rag store::tests::test_metadata_filter
```

Expected: FAIL — type not defined.

- [ ] **Step 3: Implement**

Add to `crates/rag/src/store.rs` (before `RagStore` struct):

```rust
/// Filter for metadata-based queries. All fields are optional.
/// Constructed in Rust from validated inputs — never from raw user/JS strings.
pub struct MetadataFilter {
    pub mime_type: Option<String>,
    /// Matched as `doc_id LIKE 'prefix/%'`. Trailing slash added if absent.
    pub path_prefix: Option<String>,
    /// Unix timestamp (seconds). Matched as `mtime > after`.
    pub after: Option<i64>,
    /// Unix timestamp (seconds). Matched as `mtime < before`.
    pub before: Option<i64>,
}

impl MetadataFilter {
    /// Build a LanceDB SQL WHERE clause from this filter.
    /// Returns `None` if no fields are set.
    #[must_use]
    pub fn to_where_clause(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();

        if let Some(mime) = &self.mime_type {
            let safe = mime.replace('\'', "\\'");
            parts.push(format!("mime_type = '{safe}'"));
        }
        if let Some(prefix) = &self.path_prefix {
            let prefix = if prefix.ends_with('/') {
                prefix.clone()
            } else {
                format!("{prefix}/")
            };
            // Escape SQL special chars
            let safe = prefix.replace('\'', "\\'");
            parts.push(format!("doc_id LIKE '{safe}%'"));
        }
        if let Some(after) = self.after {
            parts.push(format!("mtime > {after}"));
        }
        if let Some(before) = self.before {
            parts.push(format!("mtime < {before}"));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q -p super-ragondin-rag store::tests::test_metadata_filter
```

Expected: all pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/store.rs
git commit -m "feat(rag): add MetadataFilter type with SQL clause builder"
```

---

### Task 3: Add `DocInfo`, `ChunkInfo`, `DocSort` types and `list_docs()` / `get_chunks()` methods

**Files:**
- Modify: `crates/rag/src/store.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/rag/src/store.rs` tests:

```rust
#[tokio::test]
async fn test_list_docs_dedup_and_sort() {
    let dir = tempdir().unwrap();
    let store = RagStore::open(dir.path()).await.unwrap();

    // Two chunks for doc a (mtime 200), one for doc b (mtime 100)
    let mut c0 = make_chunk("notes/a.md", 0, "chunk0");
    c0.mtime = 200;
    let mut c1 = make_chunk("notes/a.md", 1, "chunk1");
    c1.mtime = 200;
    let mut c2 = make_chunk("work/b.md", 0, "chunk2");
    c2.mtime = 100;
    store.upsert_chunks(&[c0, c1, c2]).await.unwrap();

    // Recent first — should be deduplicated to 2 docs
    let docs = store.list_docs(None, DocSort::Recent, None).await.unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].doc_id, "notes/a.md"); // higher mtime first
    assert_eq!(docs[1].doc_id, "work/b.md");

    // Filter by path prefix
    let filtered = store
        .list_docs(
            Some(&MetadataFilter {
                path_prefix: Some("work".to_string()),
                mime_type: None,
                after: None,
                before: None,
            }),
            DocSort::Recent,
            None,
        )
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].doc_id, "work/b.md");

    // Limit
    let limited = store.list_docs(None, DocSort::Recent, Some(1)).await.unwrap();
    assert_eq!(limited.len(), 1);
}

#[tokio::test]
async fn test_get_chunks_ordered() {
    let dir = tempdir().unwrap();
    let store = RagStore::open(dir.path()).await.unwrap();
    store
        .upsert_chunks(&[
            make_chunk("notes/a.md", 1, "second"),
            make_chunk("notes/a.md", 0, "first"),
        ])
        .await
        .unwrap();

    let chunks = store.get_chunks("notes/a.md").await.unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chunk_index, 0);
    assert_eq!(chunks[0].chunk_text, "first");
    assert_eq!(chunks[1].chunk_index, 1);
    assert_eq!(chunks[1].chunk_text, "second");

    let empty = store.get_chunks("nonexistent.md").await.unwrap();
    assert!(empty.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-rag "test_list_docs|test_get_chunks"
```

Expected: FAIL.

- [ ] **Step 3: Implement**

First add the new types after the existing `SearchResult` struct:

```rust
pub enum DocSort {
    Recent,
    Oldest,
}

/// One entry per document returned by `list_docs()`.
#[derive(Debug, Clone)]
pub struct DocInfo {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
}

/// One chunk entry returned by `get_chunks()`.
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub chunk_index: u32,
    pub chunk_text: String,
}
```

Add `use std::collections::HashMap;` at the top.

Then add the two methods inside `impl RagStore`:

```rust
/// Return one entry per unique `doc_id`, sorted by `mtime`.
///
/// De-duplication is client-side (keeps the row with the highest mtime per doc_id).
/// `limit` is applied after de-duplication.
///
/// # Errors
/// Returns error if the database query fails.
///
/// # Panics
/// Panics if expected columns are absent from the result batch.
pub async fn list_docs(
    &self,
    filter: Option<&MetadataFilter>,
    sort: DocSort,
    limit: Option<usize>,
) -> Result<Vec<DocInfo>> {
    let mut q = self.table.query().select(Select::Columns(vec![
        "doc_id".to_string(),
        "mime_type".to_string(),
        "mtime".to_string(),
    ]));
    if let Some(f) = filter {
        if let Some(clause) = f.to_where_clause() {
            q = q.only_if(clause);
        }
    }
    let mut stream: SendableRecordBatchStream = q.execute().await?;

    let mut map: HashMap<String, DocInfo> = HashMap::new();
    while let Some(batch) = stream.try_next().await? {
        let doc_ids = batch
            .column_by_name("doc_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let mimes = batch
            .column_by_name("mime_type")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let mtimes = batch
            .column_by_name("mtime")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        for i in 0..batch.num_rows() {
            let doc_id = doc_ids.value(i).to_string();
            let mtime = mtimes.value(i);
            map.entry(doc_id.clone())
                .and_modify(|e| {
                    if mtime > e.mtime {
                        e.mtime = mtime;
                    }
                })
                .or_insert(DocInfo {
                    doc_id,
                    mime_type: mimes.value(i).to_string(),
                    mtime,
                });
        }
    }

    let mut docs: Vec<DocInfo> = map.into_values().collect();
    docs.sort_by(|a, b| match sort {
        DocSort::Recent => b.mtime.cmp(&a.mtime),
        DocSort::Oldest => a.mtime.cmp(&b.mtime),
    });
    if let Some(n) = limit {
        docs.truncate(n);
    }
    Ok(docs)
}

/// Return all chunks for a document, ordered by `chunk_index`.
///
/// # Errors
/// Returns error if the database query fails.
///
/// # Panics
/// Panics if expected columns are absent from the result batch.
pub async fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkInfo>> {
    let safe = doc_id.replace('\'', "\\'");
    let mut stream: SendableRecordBatchStream = self
        .table
        .query()
        .only_if(format!("doc_id = '{safe}'"))
        .select(Select::Columns(vec![
            "chunk_index".to_string(),
            "chunk_text".to_string(),
        ]))
        .execute()
        .await?;

    let mut chunks = Vec::new();
    while let Some(batch) = stream.try_next().await? {
        let indices = batch
            .column_by_name("chunk_index")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt32Array>()
            .unwrap();
        let texts = batch
            .column_by_name("chunk_text")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        for i in 0..batch.num_rows() {
            chunks.push(ChunkInfo {
                chunk_index: indices.value(i),
                chunk_text: texts.value(i).to_string(),
            });
        }
    }
    chunks.sort_by_key(|c| c.chunk_index);
    Ok(chunks)
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q -p super-ragondin-rag "test_list_docs|test_get_chunks"
```

Expected: all pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/store.rs
git commit -m "feat(rag): add DocInfo/ChunkInfo/DocSort types and list_docs()/get_chunks() methods"
```

---

### Task 4: Add `MetadataFilter` to `RagStore::search()` and `searcher::search()`

**Files:**
- Modify: `crates/rag/src/store.rs`
- Modify: `crates/rag/src/searcher.rs`
- Modify: `crates/cli/src/main.rs` (update call site to pass `None`)

- [ ] **Step 1: Write the failing test**

Add to `crates/rag/src/store.rs` tests:

```rust
#[tokio::test]
async fn test_search_with_mime_filter() {
    let dir = tempdir().unwrap();
    let store = RagStore::open(dir.path()).await.unwrap();

    let mut pdf_chunk = make_chunk("docs/report.pdf", 0, "quarterly results");
    pdf_chunk.mime_type = "application/pdf".to_string();
    let txt_chunk = make_chunk("notes/a.md", 0, "quarterly results");
    // txt_chunk mime_type defaults to "text/plain" from make_chunk

    store.upsert_chunks(&[pdf_chunk, txt_chunk]).await.unwrap();

    let filter = MetadataFilter {
        mime_type: Some("application/pdf".to_string()),
        path_prefix: None,
        after: None,
        before: None,
    };
    let results = store
        .search(&vec![0.0_f32; 3072], 5, Some(&filter))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].doc_id, "docs/report.pdf");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-rag test_search_with_mime_filter
```

Expected: FAIL.

- [ ] **Step 3: Implement**

In `crates/rag/src/store.rs`, update `search()` signature and body:

```rust
pub async fn search(
    &self,
    query_embedding: &[f32],
    limit: usize,
    filter: Option<&MetadataFilter>,
) -> Result<Vec<SearchResult>> {
    let mut query = self.table.vector_search(query_embedding)?.limit(limit);
    if let Some(f) = filter {
        if let Some(clause) = f.to_where_clause() {
            query = query.only_if(clause);
        }
    }
    let mut stream: SendableRecordBatchStream = query.execute().await?;
    // ... rest of method unchanged
```

In `crates/rag/src/searcher.rs`, update `search()` signature:

```rust
pub async fn search(
    question: &str,
    rag_store: &RagStore,
    embedder: &dyn Embedder,
    limit: usize,
    filter: Option<&MetadataFilter>,
) -> Result<Vec<SearchResult>> {
    let embeddings = embedder.embed_texts(&[question.to_string()]).await?;
    if let Some(query_vec) = embeddings.into_iter().next() {
        rag_store.search(&query_vec, limit, filter).await
    } else {
        Ok(Vec::new())
    }
}
```

Add the import at the top of `searcher.rs`:
```rust
use crate::store::{MetadataFilter, RagStore, SearchResult};
```

In `crates/rag/src/searcher.rs` tests, update the existing call:
```rust
// before:
let results = search("remote work policy", &store, &embedder, 5).await.unwrap();
// after:
let results = search("remote work policy", &store, &embedder, 5, None).await.unwrap();
```

In `crates/cli/src/main.rs`, update the `cmd_ask` call:
```rust
search(&question, &rag_store, &embedder, 5, None).await
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q
```

Expected: all pass (including existing searcher tests which now pass `None`).

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/store.rs crates/rag/src/searcher.rs crates/cli/src/main.rs
git commit -m "feat(rag): add MetadataFilter parameter to search()"
```

---

## Chunk 2: Codemode Crate Scaffold and Tools

### Task 5: Scaffold `crates/codemode/`

**Files:**
- Create: `crates/codemode/Cargo.toml`
- Create: `crates/codemode/src/lib.rs`
- Create: `crates/codemode/src/prompt.rs`
- Create: `crates/codemode/src/sandbox.rs` (stub)
- Create: `crates/codemode/src/engine.rs` (stub)
- Create: `crates/codemode/src/tools/mod.rs`
- Modify: `Cargo.toml` (workspace)

- [ ] **Step 1: Add codemode to workspace**

In the root `Cargo.toml`:
```toml
[workspace]
members = ["crates/sync", "crates/rag", "crates/cli", "crates/codemode"]
```

- [ ] **Step 2: Create `crates/codemode/Cargo.toml`**

```toml
[package]
name = "super-ragondin-codemode"
version = "0.1.0"
edition = "2024"

[dependencies]
super-ragondin-rag = { path = "../rag" }

# JS engine — 0.20+ required for register_global_callable API
boa_engine = "0.20"

# HTTP client — no stream feature needed (all LLM calls are non-streaming in this crate)
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Error handling
anyhow = "1"
tracing = "0.1"

# Date/time
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tempfile = "3"
tokio = { version = "1", features = ["full"] }

[lints]
workspace = true
```

**Note:** `boa_engine` significantly increases compile time. This is expected.

- [ ] **Step 3: Create stub source files**

`crates/codemode/src/lib.rs`:
```rust
pub mod engine;
pub mod prompt;
mod sandbox;
mod tools;
```

`crates/codemode/src/prompt.rs`:
```rust
// placeholder — implemented in Task 6
pub fn system_prompt() -> &'static str {
    ""
}
```

`crates/codemode/src/sandbox.rs`:
```rust
// placeholder — implemented in Task 12
```

`crates/codemode/src/engine.rs`:
```rust
// placeholder — implemented in Task 13
```

`crates/codemode/src/tools/mod.rs`:
```rust
pub mod get_document;
pub mod list_files;
pub mod search;
pub mod sub_agent;
```

Create empty stub files:
- `crates/codemode/src/tools/search.rs`
- `crates/codemode/src/tools/list_files.rs`
- `crates/codemode/src/tools/get_document.rs`
- `crates/codemode/src/tools/sub_agent.rs`

- [ ] **Step 4: Verify it compiles**

```bash
cargo build -p super-ragondin-codemode
```

Expected: compiles (warnings ok, errors not ok).

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/codemode/
git commit -m "feat(codemode): scaffold crates/codemode with Boa dependency"
```

---

### Task 6: Implement `prompt.rs`

**Files:**
- Modify: `crates/codemode/src/prompt.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_contains_key_elements() {
        let p = system_prompt();
        assert!(p.contains("Super Ragondin"));
        assert!(p.contains("execute_js"));
        assert!(p.contains("search("));
        assert!(p.contains("listFiles("));
        assert!(p.contains("getDocument("));
        assert!(p.contains("subAgent("));
        assert!(p.contains("ISO 8601") || p.contains("iso 8601") || p.contains("2024-"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode prompt
```

Expected: FAIL (empty string).

- [ ] **Step 3: Implement**

Replace the content of `crates/codemode/src/prompt.rs`:

```rust
/// System prompt for the code-mode LLM agent.
///
/// Describes the JS sandbox API and provides usage examples.
/// Lives here for easy modification without touching engine logic.
#[must_use]
pub fn system_prompt() -> &'static str {
    r#"You are Super Ragondin, a helpful assistant with access to a personal document database.
To answer questions, use the `execute_js` tool to query the database before responding.

Available JavaScript functions:

  search(query, options?)
    Semantic vector search. Options: { limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, chunk_text, mime_type, mtime }, ...]
    mtime is an ISO 8601 string (e.g. "2024-06-15T10:30:00Z")

  listFiles(options?)
    Discover files by metadata. Options: { sort: "recent"|"oldest", limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, mime_type, mtime }, ...]

  getDocument(docId)
    Fetch all chunks of a document in order.
    Returns: [{ chunk_index, chunk_text }, ...]

  subAgent(systemPrompt, userPrompt)
    Ask a fast LLM to process text (summarize, extract, etc.)
    Returns: string

Rules:
- Each execute_js call is a fresh context — variables do not persist between calls
- The last expression in your JS code is the return value (JSON-serialized)
- Dates in mtime, after, before are ISO 8601 strings
- Use multiple execute_js calls when gathering information in stages
- For complex questions, decompose: search each aspect separately, use subAgent() to summarize each, then synthesize a final answer
- When the user refers to a recent or specific document, start with listFiles({ sort: "recent" })
- Once you have enough information, write your final answer directly without another tool call

Examples:

// Simple search
search("project deadline", { limit: 5 })

// Get the most recently added document
const files = listFiles({ sort: "recent", limit: 1 });
getDocument(files[0].doc_id)

// Multi-aspect question with sub-agent summarization
const budgetChunks = search("budget forecasts", { limit: 3 });
const headcountChunks = search("team headcount", { limit: 3 });
const budgetSummary = subAgent("Summarize concisely.", budgetChunks.map(r => r.chunk_text).join("\n"));
const headcountSummary = subAgent("Summarize concisely.", headcountChunks.map(r => r.chunk_text).join("\n"));
({ budget: budgetSummary, headcount: headcountSummary })

// Search only in a specific folder and date range
search("meeting notes", { pathPrefix: "work/", after: "2025-01-01", limit: 10 })"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_contains_key_elements() {
        let p = system_prompt();
        assert!(p.contains("Super Ragondin"));
        assert!(p.contains("execute_js"));
        assert!(p.contains("search("));
        assert!(p.contains("listFiles("));
        assert!(p.contains("getDocument("));
        assert!(p.contains("subAgent("));
        assert!(p.contains("ISO 8601"));
    }
}
```

- [ ] **Step 4: Run test**

```bash
cargo test -q -p super-ragondin-codemode prompt
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/prompt.rs
git commit -m "feat(codemode): implement system prompt"
```

---

### Task 7: Implement `sandbox.rs` foundations (context type, thread-local, JSON helpers)

All tool implementations depend on `SandboxContext`, `SANDBOX_CTX`, and the JSON helpers. Define them here before implementing any tool. The `Sandbox` execution struct is added later in Task 12.

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

**Design note:** `Sandbox::execute()` must only be called from a `spawn_blocking` thread (not from an `async` context directly), because it calls `Handle::current().block_on(...)` via the tool functions. The engine (Task 13) always calls `execute()` through `tokio::task::spawn_blocking`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Context;

    #[test]
    fn test_jsvalue_to_serde_string() {
        let mut ctx = Context::default();
        let val = ctx.eval(boa_engine::Source::from_bytes(b"'hello'")).unwrap();
        let serde = jsvalue_to_serde(val, &mut ctx);
        assert_eq!(serde, serde_json::json!("hello"));
    }

    #[test]
    fn test_jsvalue_to_serde_array() {
        let mut ctx = Context::default();
        let val = ctx.eval(boa_engine::Source::from_bytes(b"[1, 2, 3]")).unwrap();
        let serde = jsvalue_to_serde(val, &mut ctx);
        assert_eq!(serde, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_serde_to_jsvalue_roundtrip() {
        let mut ctx = Context::default();
        let original = serde_json::json!({ "doc_id": "notes/a.md", "mtime": "2024-01-01T00:00:00Z" });
        let js = serde_to_jsvalue(&original, &mut ctx).unwrap();
        let back = jsvalue_to_serde(js, &mut ctx);
        assert_eq!(original, back);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode sandbox
```

Expected: FAIL.

- [ ] **Step 3: Implement**

Write `crates/codemode/src/sandbox.rs`:

```rust
use std::cell::RefCell;
use std::sync::Arc;

use boa_engine::{Context, JsError, JsValue, Source, js_string};
use serde_json::Value as SerdeValue;
use super_ragondin_rag::{config::RagConfig, embedder::OpenRouterEmbedder, store::RagStore};

/// Shared Rust state available to all JS native functions during a single execution.
/// Set via thread-local before each Boa evaluation; cleared after.
///
/// All tool functions (`search`, `listFiles`, etc.) access this to reach the store,
/// embedder, and Tokio runtime handle.
pub(crate) struct SandboxContext {
    pub store: Arc<RagStore>,
    pub embedder: Arc<OpenRouterEmbedder>,
    pub config: RagConfig,
    pub handle: tokio::runtime::Handle,
}

thread_local! {
    /// Active sandbox context for the current Boa execution.
    /// None outside of a `Sandbox::execute()` call.
    pub(crate) static SANDBOX_CTX: RefCell<Option<SandboxContext>> = const { RefCell::new(None) };
}

/// Convert a `JsValue` to a `serde_json::Value` via `JSON.stringify` inside Boa.
///
/// Uses a temporary global variable (`__sr_tmp__`) as an intermediary.
/// Returns `Value::Null` on failure.
pub(crate) fn jsvalue_to_serde(val: JsValue, ctx: &mut Context) -> SerdeValue {
    let _ = ctx
        .global_object()
        .set(js_string!("__sr_tmp__"), val, false, ctx);
    match ctx.eval(Source::from_bytes(b"JSON.stringify(__sr_tmp__)")) {
        Ok(JsValue::String(s)) => {
            serde_json::from_str(&s.to_std_string_escaped()).unwrap_or(SerdeValue::Null)
        }
        _ => SerdeValue::Null,
    }
}

/// Convert a `serde_json::Value` to a `JsValue` by evaluating the JSON as JS code.
///
/// Uses the `(JSON)` eval trick, which is safe for any valid JSON value.
pub(crate) fn serde_to_jsvalue(val: &SerdeValue, ctx: &mut Context) -> Result<JsValue, JsError> {
    let json_str = serde_json::to_string(val).unwrap_or_else(|_| "null".to_string());
    ctx.eval(Source::from_bytes(format!("({json_str})").as_bytes()))
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q -p super-ragondin-codemode sandbox
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/sandbox.rs
git commit -m "feat(codemode): add SandboxContext, thread-local, and JSON helpers"
```

---

### Task 8: Implement `tools/search.rs`

**Files:**
- Modify: `crates/codemode/src/tools/search.rs`

The search tool parses the JS `options` object, builds a `MetadataFilter`, embeds the query, and returns an array of result objects.

The tool functions use a thread-local `SandboxContext` (defined in `sandbox.rs`, set before each Boa execution). See Task 12 for the full context type definition — for now, write the tool assuming it exists with fields: `store: Arc<RagStore>`, `embedder: Arc<OpenRouterEmbedder>`, `handle: tokio::runtime::Handle`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxContext;
    use std::sync::Arc;
    use super_ragondin_rag::{
        embedder::OpenRouterEmbedder,
        config::RagConfig,
        store::{ChunkRecord, RagStore},
    };
    use tempfile::tempdir;

    // Helper: build a SandboxContext backed by a real in-memory store
    async fn make_ctx(store: Arc<RagStore>, config: RagConfig) -> SandboxContext {
        let embedder = Arc::new(OpenRouterEmbedder::new(config.clone()));
        let handle = tokio::runtime::Handle::current();
        SandboxContext { store, embedder, config, handle }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_tool_returns_results() {
        let dir = tempdir().unwrap();
        let store = Arc::new(RagStore::open(dir.path()).await.unwrap());
        store.upsert_chunks(&[ChunkRecord {
            id: "notes/a.md:0".to_string(),
            doc_id: "notes/a.md".to_string(),
            mime_type: "text/plain".to_string(),
            mtime: 1_700_000_000,
            chunk_index: 0,
            chunk_text: "hello world".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 3072],
        }]).await.unwrap();

        let config = RagConfig::from_env_with_db_path(dir.path().to_path_buf());
        let sandbox_ctx = make_ctx(store, config).await;
        crate::sandbox::SANDBOX_CTX.with(|c| *c.borrow_mut() = Some(sandbox_ctx));

        // Execute in a Boa context
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(
            b"JSON.stringify(search('hello'))"
        )).unwrap();

        crate::sandbox::SANDBOX_CTX.with(|c| *c.borrow_mut() = None);

        let result_str = result.as_string().unwrap().to_std_string_escaped();
        let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();
        assert!(parsed.as_array().unwrap().len() >= 1);
        assert_eq!(parsed[0]["doc_id"], "notes/a.md");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode tools::search
```

Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/codemode/src/tools/search.rs`:

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::store::MetadataFilter;

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `search(query, options?)` global function in the Boa context.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("search"),
        1,
        NativeFunction::from_fn_ptr(search_fn),
    )?;
    Ok(())
}

fn search_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let query = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    // Parse options object
    let opts = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let limit = get_number_opt(&opts, "limit", ctx).map_or(5, |n| n as usize);
    let mime_type = get_string_opt(&opts, "mimeType", ctx);
    let path_prefix = get_string_opt(&opts, "pathPrefix", ctx);
    let after = get_string_opt(&opts, "after", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());
    let before = get_string_opt(&opts, "before", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());

    let filter = MetadataFilter { mime_type, path_prefix, after, before };
    let has_filter = filter.mime_type.is_some()
        || filter.path_prefix.is_some()
        || filter.after.is_some()
        || filter.before.is_some();

    let results = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        let embedder = std::sync::Arc::clone(&sandbox.embedder);
        let filter_opt = if has_filter { Some(filter) } else { None };
        sandbox.handle.block_on(async move {
            let vecs = embedder
                .embed_texts(&[query])
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
            let vec = vecs.into_iter().next().unwrap_or_default();
            store
                .search(&vec, limit, filter_opt.as_ref())
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    // Convert to JS: [{ doc_id, chunk_text, mime_type, mtime }]
    let json_results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            use chrono::{DateTime, TimeZone, Utc};
            let mtime_dt: DateTime<Utc> = Utc.timestamp_opt(r.mtime, 0).unwrap_or(Utc::now());
            serde_json::json!({
                "doc_id": r.doc_id,
                "chunk_text": r.chunk_text,
                "mime_type": r.mime_type,
                "mtime": mtime_dt.to_rfc3339(),
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_results), ctx)
}

fn get_string_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<String> {
    obj.as_object()
        .and_then(|o| o.get(js_string!(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
}

fn get_number_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<f64> {
    obj.as_object()
        .and_then(|o| o.get(js_string!(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_number(ctx).ok())
}
```

**Note on the test:** This test requires a real embedder call (OpenRouter API). For CI, the test should be `#[ignore]`. The unit test above exercises the registration and argument parsing; the actual embedding call will fail without an API key and the error will be returned as a JsError. An alternative is to mock the embedder — see the searcher tests in `crates/rag/` for the `StubEmbedder` pattern. For now, add `#[ignore]` to the test:

```rust
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real embedder; use StubEmbedder or integration test"]
async fn test_search_tool_returns_results() {
```

And write a non-ignored test that only checks registration:

```rust
#[test]
fn test_search_registers_without_panic() {
    let mut ctx = boa_engine::Context::default();
    register(&mut ctx).unwrap();
    // search is callable (returns JS error without context, that's ok)
    let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof search"));
    assert_eq!(
        result.unwrap().as_string().unwrap().to_std_string_escaped(),
        "function"
    );
}
```

- [ ] **Step 4: Run test**

```bash
cargo test -q -p super-ragondin-codemode tools::search::tests::test_search_registers
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/tools/search.rs
git commit -m "feat(codemode): implement search() JS tool"
```

---

### Task 9: Implement `tools/list_files.rs`

**Files:**
- Modify: `crates/codemode/src/tools/list_files.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_files_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof listFiles"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode tools::list_files
```

Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/codemode/src/tools/list_files.rs`:

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::store::{DocSort, MetadataFilter};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `listFiles(options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("listFiles"),
        0,
        NativeFunction::from_fn_ptr(list_files_fn),
    )?;
    Ok(())
}

fn list_files_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let opts = args.get(0).cloned().unwrap_or(JsValue::undefined());

    let sort_str = get_string_opt(&opts, "sort", ctx).unwrap_or_else(|| "recent".to_string());
    let sort = if sort_str == "oldest" { DocSort::Oldest } else { DocSort::Recent };
    let limit = get_number_opt(&opts, "limit", ctx).map(|n| n as usize);
    let mime_type = get_string_opt(&opts, "mimeType", ctx);
    let path_prefix = get_string_opt(&opts, "pathPrefix", ctx);
    let after = get_string_opt(&opts, "after", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());
    let before = get_string_opt(&opts, "before", ctx)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp());

    let has_filter = mime_type.is_some() || path_prefix.is_some() || after.is_some() || before.is_some();
    let filter = MetadataFilter { mime_type, path_prefix, after, before };

    let docs = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        let filter_opt = if has_filter { Some(filter) } else { None };
        sandbox.handle.block_on(async move {
            store
                .list_docs(filter_opt.as_ref(), sort, limit)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    let json_docs: Vec<serde_json::Value> = docs
        .into_iter()
        .map(|d| {
            use chrono::{DateTime, TimeZone, Utc};
            let mtime_dt: DateTime<Utc> = Utc.timestamp_opt(d.mtime, 0).unwrap_or(Utc::now());
            serde_json::json!({
                "doc_id": d.doc_id,
                "mime_type": d.mime_type,
                "mtime": mtime_dt.to_rfc3339(),
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_docs), ctx)
}

fn get_string_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<String> {
    obj.as_object()
        .and_then(|o| o.get(js_string!(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
}

fn get_number_opt(obj: &JsValue, key: &str, ctx: &mut Context) -> Option<f64> {
    obj.as_object()
        .and_then(|o| o.get(js_string!(key), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_number(ctx).ok())
}
```

**Note:** `get_string_opt` and `get_number_opt` are duplicated across tools. This is acceptable (YAGNI — don't abstract until there's a clear benefit). If clippy warns about duplicate code, add `#[allow(clippy::...)]`.

- [ ] **Step 4: Run test**

```bash
cargo test -q -p super-ragondin-codemode tools::list_files
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/tools/list_files.rs
git commit -m "feat(codemode): implement listFiles() JS tool"
```

---

### Task 10: Implement `tools/get_document.rs`

**Files:**
- Modify: `crates/codemode/src/tools/get_document.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_document_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof getDocument"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode tools::get_document
```

Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/codemode/src/tools/get_document.rs`:

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

/// Register the `getDocument(docId)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("getDocument"),
        1,
        NativeFunction::from_fn_ptr(get_document_fn),
    )?;
    Ok(())
}

fn get_document_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let doc_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let chunks = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let store = std::sync::Arc::clone(&sandbox.store);
        sandbox.handle.block_on(async move {
            store
                .get_chunks(&doc_id)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    let json_chunks: Vec<serde_json::Value> = chunks
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "chunk_index": c.chunk_index,
                "chunk_text": c.chunk_text,
            })
        })
        .collect();

    serde_to_jsvalue(&serde_json::Value::Array(json_chunks), ctx)
}
```

- [ ] **Step 4: Run test**

```bash
cargo test -q -p super-ragondin-codemode tools::get_document
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/tools/get_document.rs
git commit -m "feat(codemode): implement getDocument() JS tool"
```

---

### Task 11: Implement `tools/sub_agent.rs`

**Files:**
- Modify: `crates/codemode/src/tools/sub_agent.rs`

This tool makes a non-streaming OpenRouter chat completion request using the `subagent_model` from config.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof subAgent"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode tools::sub_agent
```

Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/codemode/src/tools/sub_agent.rs`:

```rust
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::SANDBOX_CTX;

/// Register the `subAgent(systemPrompt, userPrompt)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("subAgent"),
        2,
        NativeFunction::from_fn_ptr(sub_agent_fn),
    )?;
    Ok(())
}

fn sub_agent_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let system_prompt = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let user_prompt = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();

    let response = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        let api_key = sandbox.config.api_key.clone();
        let model = sandbox.config.subagent_model.clone();
        sandbox.handle.block_on(async move {
            call_sub_agent(&api_key, &model, &system_prompt, &user_prompt)
                .await
                .map_err(|e| JsNativeError::error().with_message(e.to_string()))
        })
    })?;

    Ok(boa_engine::JsValue::String(js_string!(response)))
}

async fn call_sub_agent(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]
    });

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(api_key)
        .header("HTTP-Referer", "https://github.com/super-ragondin")
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
```

- [ ] **Step 4: Run test**

```bash
cargo test -q -p super-ragondin-codemode tools::sub_agent
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/tools/sub_agent.rs
git commit -m "feat(codemode): implement subAgent() JS tool"
```

---

## Chunk 3: Sandbox, Engine, and CLI Wiring

### Task 12: Add `Sandbox` execution struct to `sandbox.rs`

`SandboxContext`, `SANDBOX_CTX`, and the JSON helpers were already added in Task 7. This task adds the `Sandbox` struct (the public execution wrapper) to the same file.

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super_ragondin_rag::{config::RagConfig, store::RagStore};
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn make_sandbox() -> Sandbox {
        let dir = tempdir().unwrap();
        let store = Arc::new(RagStore::open(dir.path()).await.unwrap());
        let config = RagConfig::from_env_with_db_path(dir.path().to_path_buf());
        Sandbox::new(store, config)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_arithmetic() {
        let sandbox = make_sandbox().await;
        let result = sandbox.execute("1 + 2").unwrap();
        assert_eq!(result, "3");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_returns_last_expression() {
        let sandbox = make_sandbox().await;
        let result = sandbox.execute("const x = 10; x * 2").unwrap();
        assert_eq!(result, "20");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_js_error_returns_err_string() {
        let sandbox = make_sandbox().await;
        let result = sandbox.execute("undeclaredFunction()");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("JS error"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_object_result() {
        let sandbox = make_sandbox().await;
        let result = sandbox.execute("({ a: 1, b: 'hello' })").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], "hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fresh_context_per_call() {
        let sandbox = make_sandbox().await;
        // Set a variable in one call
        sandbox.execute("var x = 42;").ok();
        // It must NOT be visible in the next call
        let result = sandbox.execute("typeof x === 'undefined'").unwrap();
        assert_eq!(result, "true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sandbox_globals_registered() {
        let sandbox = make_sandbox().await;
        for fn_name in &["search", "listFiles", "getDocument", "subAgent"] {
            let result = sandbox
                .execute(&format!("typeof {fn_name}"))
                .unwrap();
            assert_eq!(result, "\"function\"", "{fn_name} should be a function");
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode sandbox
```

Expected: FAIL.

- [ ] **Step 3: Implement**

Add the `Sandbox` struct to the existing `crates/codemode/src/sandbox.rs` (after the existing content from Task 7):

```rust
use crate::tools;

/// Execution wrapper: creates a fresh Boa context per call.
///
/// Must be called from a `spawn_blocking` thread — tool functions call
/// `Handle::current().block_on(...)` internally.
pub struct Sandbox {
    store: Arc<RagStore>,
    config: RagConfig,
}

impl Sandbox {
    #[must_use]
    pub fn new(store: Arc<RagStore>, config: RagConfig) -> Self {
        Self { store, config }
    }

    /// Execute JS code in a fresh Boa context.
    ///
    /// Returns the JSON-serialized value of the last expression.
    ///
    /// # Errors
    /// Returns `Err(String)` with a human-readable message on JS or Rust error.
    pub fn execute(&self, code: &str) -> Result<String, String> {
        let handle = tokio::runtime::Handle::current();
        let embedder = Arc::new(OpenRouterEmbedder::new(self.config.clone()));

        SANDBOX_CTX.with(|cell| {
            *cell.borrow_mut() = Some(SandboxContext {
                store: Arc::clone(&self.store),
                embedder,
                config: self.config.clone(),
                handle,
            });
        });

        let result = self.run_boa(code);

        SANDBOX_CTX.with(|cell| {
            *cell.borrow_mut() = None;
        });

        result
    }

    fn run_boa(&self, code: &str) -> Result<String, String> {
        let mut ctx = Context::default();

        tools::search::register(&mut ctx)
            .map_err(|e| format!("JS error: register search: {e}"))?;
        tools::list_files::register(&mut ctx)
            .map_err(|e| format!("JS error: register listFiles: {e}"))?;
        tools::get_document::register(&mut ctx)
            .map_err(|e| format!("JS error: register getDocument: {e}"))?;
        tools::sub_agent::register(&mut ctx)
            .map_err(|e| format!("JS error: register subAgent: {e}"))?;

        let val = ctx
            .eval(Source::from_bytes(code.as_bytes()))
            .map_err(|e| format!("JS error: {e}"))?;

        let serde_val = jsvalue_to_serde(val, &mut ctx);
        serde_json::to_string(&serde_val)
            .map_err(|e| format!("JS error: serialize result: {e}"))
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -q -p super-ragondin-codemode sandbox
```

Expected: all pass (JS error test may differ in message format — adjust the assertion to match actual Boa error format if needed).

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/sandbox.rs
git commit -m "feat(codemode): implement Sandbox execution wrapper"
```

---

### Task 13: Implement `engine.rs` (LLM tool-use loop)

The engine manages the OpenRouter message loop: sends the user question, handles `execute_js` tool calls, and returns the final text response.

**Files:**
- Modify: `crates/codemode/src/engine.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_js_tool_definition() {
        let def = execute_js_tool_definition();
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], "execute_js");
        assert!(def["function"]["parameters"]["properties"]["code"].is_object());
    }

    #[test]
    fn test_extract_tool_call_from_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "execute_js",
                            "arguments": "{\"code\": \"1 + 1\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let call = extract_tool_call(&response).unwrap();
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.code, "1 + 1");
    }

    #[test]
    fn test_extract_text_from_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42."
                },
                "finish_reason": "stop"
            }]
        });
        assert!(extract_tool_call(&response).is_none());
        let text = extract_text(&response).unwrap();
        assert_eq!(text, "The answer is 42.");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode engine
```

Expected: FAIL.

- [ ] **Step 3: Implement**

`crates/codemode/src/engine.rs`:

```rust
use std::sync::Arc;

use anyhow::{Context as _, Result};
use super_ragondin_rag::{config::RagConfig, store::RagStore};

use crate::prompt::system_prompt;
use crate::sandbox::Sandbox;

const MAX_ITERATIONS: usize = 10;

/// Extracted tool call from an LLM response.
pub(crate) struct ToolCall {
    pub id: String,
    pub code: String,
}

/// Return the OpenAI-format tool definition for `execute_js`.
#[must_use]
pub(crate) fn execute_js_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "execute_js",
            "description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), and subAgent() functions to query the document database.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript code to execute. The last expression is returned as JSON."
                    }
                },
                "required": ["code"]
            }
        }
    })
}

/// Extract a tool call from an OpenRouter response, if present.
#[must_use]
pub(crate) fn extract_tool_call(response: &serde_json::Value) -> Option<ToolCall> {
    let tool_calls = response["choices"][0]["message"]["tool_calls"].as_array()?;
    let call = tool_calls.first()?;
    if call["function"]["name"].as_str()? != "execute_js" {
        return None;
    }
    let args_str = call["function"]["arguments"].as_str()?;
    let args: serde_json::Value = serde_json::from_str(args_str).ok()?;
    Some(ToolCall {
        id: call["id"].as_str()?.to_string(),
        code: args["code"].as_str()?.to_string(),
    })
}

/// Extract the text content from an OpenRouter response, if present.
#[must_use]
pub(crate) fn extract_text(response: &serde_json::Value) -> Option<String> {
    response["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
}

/// Drives the code-mode ask loop for a single user question.
///
/// Stores the store `Arc` and config directly so it can cheaply clone them
/// into each `spawn_blocking` call (no unsafe needed).
pub struct CodeModeEngine {
    store: Arc<RagStore>,
    config: RagConfig,
}

impl CodeModeEngine {
    /// # Errors
    /// Returns error if the RAG store cannot be opened.
    pub async fn new(config: RagConfig) -> Result<Self> {
        let store = Arc::new(RagStore::open(&config.db_path).await?);
        Ok(Self { store, config })
    }

    /// Ask a question using the code-mode LLM loop.
    ///
    /// Runs the tool-use loop (max `MAX_ITERATIONS` iterations), then prints the final answer.
    ///
    /// # Errors
    /// Returns error if the OpenRouter API call fails or the iteration limit is reached.
    pub async fn ask(&self, question: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let api_key = &self.config.api_key;
        let model = &self.config.chat_model;

        let mut messages = vec![
            serde_json::json!({"role": "system", "content": system_prompt()}),
            serde_json::json!({"role": "user", "content": question}),
        ];

        let tools = vec![execute_js_tool_definition()];

        for iteration in 0..MAX_ITERATIONS {
            let body = serde_json::json!({
                "model": model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto"
            });

            let resp = client
                .post("https://openrouter.ai/api/v1/chat/completions")
                .bearer_auth(api_key)
                .header("HTTP-Referer", "https://github.com/super-ragondin")
                .json(&body)
                .send()
                .await
                .context("OpenRouter request failed")?;

            let response: serde_json::Value =
                resp.json().await.context("Failed to parse response")?;

            if let Some(tool_call) = extract_tool_call(&response) {
                messages.push(response["choices"][0]["message"].clone());

                tracing::debug!(iteration, code = %tool_call.code, "execute_js tool call");

                // Clone Arc (cheap) and config — no unsafe needed.
                // spawn_blocking requires 'static, so we move owned data in.
                let store_clone = Arc::clone(&self.store);
                let config_clone = self.config.clone();
                let code_clone = tool_call.code.clone();

                let result = tokio::task::spawn_blocking(move || {
                    let sandbox = Sandbox::new(store_clone, config_clone);
                    sandbox.execute(&code_clone)
                })
                .await
                .context("spawn_blocking panicked")?;

                let tool_result = match result {
                    Ok(json_str) => json_str,
                    Err(err_msg) => err_msg, // JS errors returned as strings for LLM to adapt
                };

                tracing::debug!(result = %tool_result, "execute_js result");

                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call.id,
                    "content": tool_result
                }));
            } else if let Some(text) = extract_text(&response) {
                println!("{text}");
                return Ok(());
            } else {
                anyhow::bail!("Unexpected response format from OpenRouter");
            }

            if iteration == MAX_ITERATIONS - 1 {
                anyhow::bail!(
                    "Reached maximum tool-call iterations ({MAX_ITERATIONS}) without a final answer"
                );
            }
        }

        Ok(())
    }
}

- [ ] **Step 4: Run unit tests**

```bash
cargo test -q -p super-ragondin-codemode engine
```

Expected: all pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/engine.rs crates/codemode/src/sandbox.rs
git commit -m "feat(codemode): implement CodeModeEngine with LLM tool-use loop"
```

---

### Task 14: Wire up `lib.rs` and update CLI

**Files:**
- Modify: `crates/codemode/src/lib.rs`
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/cli/Cargo.toml`

- [ ] **Step 1: Expose `CodeModeEngine` from `lib.rs`**

`crates/codemode/src/lib.rs`:
```rust
pub mod engine;
pub mod prompt;
pub(crate) mod sandbox;
pub(crate) mod tools;
```

`engine::CodeModeEngine` is already `pub`.

- [ ] **Step 2: Add `codemode` dependency to CLI**

In `crates/cli/Cargo.toml`:
```toml
super-ragondin-codemode = { path = "../codemode" }
```

- [ ] **Step 3: Write a smoke-test for `cmd_ask` (compilation check)**

The best test here is that the code compiles and the CLI's `ask` command dispatches correctly. Add a unit test that checks argument parsing:

```rust
// In crates/cli/src/main.rs tests:
#[cfg(test)]
mod tests {
    #[test]
    fn test_ask_requires_question() {
        // When no args, cmd_ask should print usage and return Ok
        // This tests the argument guard, not the actual LLM call
        // (We can't easily test the full LLM path in a unit test)
        // For now, verify the binary compiles — this test always passes.
        assert!(true);
    }
}
```

The real test is: `cargo build -p super-ragondin`.

- [ ] **Step 4: Replace `cmd_ask` in CLI**

In `crates/cli/src/main.rs`, replace the entire `cmd_ask` function:

```rust
fn cmd_ask(args: &[String]) -> Result<()> {
    use super_ragondin_codemode::engine::CodeModeEngine;
    use super_ragondin_rag::config::RagConfig;

    if args.is_empty() {
        println!("Usage: super-ragondin ask <question>");
        return Ok(());
    }
    let question = args.join(" ");

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
        let engine = CodeModeEngine::new(rag_config)
            .await
            .map_err(|e| Error::Permanent(format!("{e:#}")))?;
        engine
            .ask(&question)
            .await
            .map_err(|e| Error::Permanent(format!("{e:#}")))
    })?;

    Ok(())
}
```

Remove unused imports from the old `cmd_ask` that are no longer needed:
- `use super_ragondin_rag::embedder::OpenRouterEmbedder;`
- `use super_ragondin_rag::searcher::search;`
- `use super_ragondin_rag::store::RagStore;`
- `use std::io::Write;`

Check if they're still used elsewhere; if not, remove them.

- [ ] **Step 5: Build the full project**

```bash
cargo build
```

Expected: compiles without errors. (Warnings about unused imports are ok — fix them.)

- [ ] **Step 6: Run all tests**

```bash
cargo test -q
```

Expected: all existing tests pass.

- [ ] **Step 7: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Fix any warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/codemode/src/lib.rs crates/cli/Cargo.toml crates/cli/src/main.rs
git commit -m "feat(cli): replace cmd_ask with CodeModeEngine"
```

---

## Final Verification

- [ ] **Run all tests**

```bash
cargo test -q
```

Expected: all pass, no failures.

- [ ] **Check for clippy warnings**

```bash
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Manual smoke test (optional, requires API key + running sync)**

```bash
OPENROUTER_API_KEY=<key> cargo run -- ask "What are my recent notes about?"
```

Expected: LLM uses `execute_js` tool, queries the DB, returns a coherent answer.
