# RAG System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add automatic RAG indexing of synced files and a `super-ragondin ask <question>` CLI command that returns a generated answer with source references.

**Architecture:** The `crates/rag/` crate handles all RAG logic: MIME detection, text extraction, chunking, embedding (via OpenRouter), and LanceDB storage. The CLI's sync loop calls `indexer::reconcile()` after each cycle to keep the index in sync. The `ask` command embeds the question, retrieves top-5 chunks, and streams a chat completion answer.

**Tech Stack:** `lancedb` (embedded vector DB), `chonkie` + `tiktoken-rs` (chunking), `reqwest` (OpenRouter HTTP), `pdf-extract` (PDF text), `calamine` (XLSX), `zip` + `quick-xml` (DOCX/ODT), `infer` (MIME detection), `base64` + `pdfium-render` (image/scanned PDF).

**Spec:** `docs/superpowers/specs/2026-03-10-rag-system-design.md`

---

## File Structure

### New files — `crates/rag/src/`

| File | Responsibility |
|---|---|
| `lib.rs` | Public re-exports: `RagConfig`, `RagStore`, `Indexer`, `Searcher` |
| `config.rs` | `RagConfig` — loads env vars, holds model names + DB path |
| `store.rs` | `RagStore` — LanceDB wrapper: schema, upsert, delete-by-doc, vector search |
| `embedder.rs` | `Embedder` trait + `OpenRouterEmbedder` — text embeddings + vision descriptions |
| `extractor/mod.rs` | `extract(path, mime) -> Result<String>` — dispatch by MIME |
| `extractor/plaintext.rs` | UTF-8 file read |
| `extractor/pdf.rs` | `pdf-extract`; scanned fallback via `pdfium-render` → base64 image |
| `extractor/office.rs` | DOCX/ODT via `zip` + `quick-xml`; XLSX via `calamine` |
| `extractor/image.rs` | Read file → base64 — callers pass to `Embedder::describe_image` |
| `chunker.rs` | `chunk(text, mime) -> Vec<String>` — selects chonkie strategy by MIME |
| `indexer.rs` | `reconcile(store, sync_dir, rag_store, embedder)` — diff + index |
| `searcher.rs` | `search(question, rag_store, embedder, limit) -> Vec<SearchResult>` |

### Modified files

| File | Change |
|---|---|
| `crates/rag/Cargo.toml` | Add all RAG dependencies |
| `crates/cli/src/main.rs` | Add `ask` subcommand; call `reconcile()` after each sync cycle |

---

## Chunk 1: Foundation — Config, LanceDB Store

### Task 1: Add dependencies to `crates/rag/Cargo.toml`

**Files:**
- Modify: `crates/rag/Cargo.toml`

- [ ] **Step 1: Add dependencies**

Replace `crates/rag/Cargo.toml` with:

```toml
[package]
name = "super-ragondin-rag"
version = "0.1.0"
edition = "2024"

[dependencies]
super-ragondin-sync = { path = "../sync" }

# Vector DB
lancedb = "0.20"
arrow-array = "54"
arrow-schema = "54"

# Chunking
chonkie = { version = "0.1.1", features = ["tiktoken-rs"] }

# HTTP client
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# MIME detection
infer = "0.16"

# PDF extraction
pdf-extract = "0.7"

# PDF rendering (for scanned PDF fallback)
pdfium-render = { version = "0.8", features = ["thread_safe"] }

# Office document parsing
calamine = { version = "0.26", features = ["dates"] }
zip = "2"
quick-xml = { version = "0.37", features = ["serialize"] }

# Image / base64
base64 = "0.22"
image = "0.25"

# Async utilities
async-trait = "0.1"
futures = "0.3"

# Error handling + logging
anyhow = "1"
tracing = "0.1"
thiserror = "2"

[dev-dependencies]
tempfile = "3"

[lints]
workspace = true
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p super-ragondin-rag
```

Expected: compiles (empty lib.rs, no warnings about unused deps yet).

- [ ] **Step 3: Commit**

```bash
git add crates/rag/Cargo.toml
git commit -m "feat(rag): add dependencies"
```

---

### Task 2: Implement `config.rs`

**Files:**
- Create: `crates/rag/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the bottom of `crates/rag/src/config.rs` (create file first with empty content):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        // Unset vars to test defaults
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("OPENROUTER_EMBED_MODEL");
            std::env::remove_var("OPENROUTER_VISION_MODEL");
            std::env::remove_var("OPENROUTER_CHAT_MODEL");
        }
        let config = RagConfig::from_env_with_db_path(std::path::PathBuf::from("/tmp/test.db"));
        assert_eq!(config.embed_model, "openai/text-embedding-3-large");
        assert_eq!(config.vision_model, "google/gemini-2.5-flash");
        assert_eq!(config.chat_model, "mistralai/mistral-small-3.2-24b-instruct");
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn test_config_from_env() {
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", "test-key");
            std::env::set_var("OPENROUTER_EMBED_MODEL", "custom/model");
        }
        let config = RagConfig::from_env_with_db_path(std::path::PathBuf::from("/tmp/test.db"));
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.embed_model, "custom/model");
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
            std::env::remove_var("OPENROUTER_EMBED_MODEL");
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag config -- --nocapture
```

Expected: FAIL — `RagConfig` not defined.

- [ ] **Step 3: Implement `config.rs`**

```rust
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RagConfig {
    pub api_key: String,
    pub embed_model: String,
    pub vision_model: String,
    pub chat_model: String,
    pub db_path: PathBuf,
}

impl RagConfig {
    pub fn from_env_with_db_path(db_path: PathBuf) -> Self {
        Self {
            api_key: std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
            embed_model: std::env::var("OPENROUTER_EMBED_MODEL")
                .unwrap_or_else(|_| "openai/text-embedding-3-large".to_string()),
            vision_model: std::env::var("OPENROUTER_VISION_MODEL")
                .unwrap_or_else(|_| "google/gemini-2.5-flash".to_string()),
            chat_model: std::env::var("OPENROUTER_CHAT_MODEL")
                .unwrap_or_else(|_| "mistralai/mistral-small-3.2-24b-instruct".to_string()),
            db_path,
        }
    }
}

// ... tests below
```

- [ ] **Step 4: Wire into `lib.rs`**

```rust
// crates/rag/src/lib.rs
pub mod config;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag config
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/config.rs crates/rag/src/lib.rs
git commit -m "feat(rag): add RagConfig"
```

---

### Task 3: Implement `store.rs` — LanceDB wrapper

**Files:**
- Create: `crates/rag/src/store.rs`

The store manages one LanceDB table `chunks` with columns: `id` (String), `doc_id` (String), `mime_type` (String), `mtime` (Int64), `chunk_index` (UInt32), `chunk_text` (String), `md5sum` (String), `embedding` (FixedSizeList<Float32, 3072>).

The `md5sum` column is used by the indexer to detect stale entries without a separate tracking table.

> **Note:** Verify exact LanceDB Rust API against https://docs.rs/lancedb before implementing. The API below reflects lancedb ~0.20; method signatures may differ slightly.

- [ ] **Step 1: Write the failing test**

```rust
// At the bottom of store.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_chunk(doc_id: &str, chunk_index: u32, text: &str) -> ChunkRecord {
        ChunkRecord {
            id: format!("{doc_id}:{chunk_index}"),
            doc_id: doc_id.to_string(),
            mime_type: "text/plain".to_string(),
            mtime: 1_700_000_000,
            chunk_index,
            chunk_text: text.to_string(),
            md5sum: "abc123".to_string(),
            embedding: vec![0.0_f32; 3072],
        }
    }

    #[tokio::test]
    async fn test_upsert_and_list_indexed() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        let chunk = make_chunk("notes/test.md", 0, "hello world");
        store.upsert_chunks(&[chunk]).await.unwrap();

        let indexed = store.list_indexed().await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/test.md");
        assert_eq!(indexed[0].md5sum, "abc123");
    }

    #[tokio::test]
    async fn test_delete_by_doc() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        store.upsert_chunks(&[make_chunk("notes/a.md", 0, "aaa")]).await.unwrap();
        store.upsert_chunks(&[make_chunk("notes/b.md", 0, "bbb")]).await.unwrap();
        store.delete_doc("notes/a.md").await.unwrap();

        let indexed = store.list_indexed().await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/b.md");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag store
```

Expected: FAIL — `RagStore` not defined.

- [ ] **Step 3: Implement `store.rs`**

```rust
use std::path::Path;
use std::sync::Arc;
use anyhow::Result;
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator,
    StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::{connect, Connection, Table};
use lancedb::query::QueryBase;

const TABLE_NAME: &str = "chunks";
const EMBED_DIM: i32 = 3072;

/// A single indexed chunk stored in LanceDB.
pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_index: u32,
    pub chunk_text: String,
    pub md5sum: String,
    pub embedding: Vec<f32>,
}

/// Lightweight summary returned by `list_indexed` — one entry per unique doc_id.
pub struct IndexedDoc {
    pub doc_id: String,
    pub md5sum: String,
}

fn chunks_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("mtime", DataType::Int64, false),
        Field::new("chunk_index", DataType::UInt32, false),
        Field::new("chunk_text", DataType::Utf8, false),
        Field::new("md5sum", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBED_DIM,
            ),
            false,
        ),
    ]))
}

pub struct RagStore {
    table: Table,
}

impl RagStore {
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db: Connection = connect(db_path.to_str().unwrap()).execute().await?;
        let table_names = db.table_names().execute().await?;
        let table = if table_names.contains(&TABLE_NAME.to_string()) {
            db.open_table(TABLE_NAME).execute().await?
        } else {
            // Create empty table with schema
            let schema = chunks_schema();
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
            db.create_table(TABLE_NAME, Box::new(batches)).execute().await?
        };
        Ok(Self { table })
    }

    /// Insert or replace chunks. Caller must delete old chunks first for updates.
    pub async fn upsert_chunks(&self, chunks: &[ChunkRecord]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let schema = chunks_schema();
        let n = chunks.len();

        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.doc_id.as_str()).collect();
        let mimes: Vec<&str> = chunks.iter().map(|c| c.mime_type.as_str()).collect();
        let mtimes: Vec<i64> = chunks.iter().map(|c| c.mtime).collect();
        let indices: Vec<u32> = chunks.iter().map(|c| c.chunk_index).collect();
        let texts: Vec<&str> = chunks.iter().map(|c| c.chunk_text.as_str()).collect();
        let md5sums: Vec<&str> = chunks.iter().map(|c| c.md5sum.as_str()).collect();

        // Build flat f32 values for FixedSizeList
        let flat_embeddings: Vec<f32> = chunks.iter().flat_map(|c| c.embedding.iter().copied()).collect();
        let embedding_values = Arc::new(Float32Array::from(flat_embeddings));
        let embedding_col = Arc::new(FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            EMBED_DIM,
            embedding_values,
            None,
        )?);

        let batch = RecordBatch::try_new(schema.clone(), vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(StringArray::from(doc_ids)),
            Arc::new(StringArray::from(mimes)),
            Arc::new(Int64Array::from(mtimes)),
            Arc::new(UInt32Array::from(indices)),
            Arc::new(StringArray::from(texts)),
            Arc::new(StringArray::from(md5sums)),
            embedding_col,
        ])?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        self.table.add(Box::new(batches)).execute().await?;
        Ok(())
    }

    /// Delete all chunks for a given doc_id.
    pub async fn delete_doc(&self, doc_id: &str) -> Result<()> {
        // Escape single quotes in doc_id
        let safe = doc_id.replace('\'', "\\'");
        self.table.delete(&format!("doc_id = '{safe}'")).await?;
        Ok(())
    }

    /// Return one entry per unique (doc_id, md5sum) pair — used for reconciliation.
    pub async fn list_indexed(&self) -> Result<Vec<IndexedDoc>> {
        use futures::TryStreamExt;
        let mut stream = self.table
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "doc_id".to_string(),
                "md5sum".to_string(),
            ]))
            .execute()
            .await?;

        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let doc_ids = batch.column_by_name("doc_id").unwrap()
                .as_any().downcast_ref::<StringArray>().unwrap();
            let md5sums = batch.column_by_name("md5sum").unwrap()
                .as_any().downcast_ref::<StringArray>().unwrap();
            for i in 0..batch.num_rows() {
                let doc_id = doc_ids.value(i).to_string();
                if seen.insert(doc_id.clone()) {
                    result.push(IndexedDoc {
                        doc_id,
                        md5sum: md5sums.value(i).to_string(),
                    });
                }
            }
        }
        Ok(result)
    }

    /// Vector similarity search — returns top-k chunks with their text and doc metadata.
    pub async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        use futures::TryStreamExt;
        let mut stream = self.table
            .vector_search(query_embedding)?
            .limit(limit)
            .execute()
            .await?;

        let mut results = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let doc_ids = batch.column_by_name("doc_id").unwrap()
                .as_any().downcast_ref::<StringArray>().unwrap();
            let mimes = batch.column_by_name("mime_type").unwrap()
                .as_any().downcast_ref::<StringArray>().unwrap();
            let mtimes = batch.column_by_name("mtime").unwrap()
                .as_any().downcast_ref::<Int64Array>().unwrap();
            let texts = batch.column_by_name("chunk_text").unwrap()
                .as_any().downcast_ref::<StringArray>().unwrap();
            for i in 0..batch.num_rows() {
                results.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    mime_type: mimes.value(i).to_string(),
                    mtime: mtimes.value(i),
                    chunk_text: texts.value(i).to_string(),
                });
            }
        }
        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_text: String,
}
```

> **Note:** All chunks for a given `doc_id` share the same `md5sum` — this is an invariant maintained by `upsert_chunks` (caller deletes before upserting). The deduplication in `list_indexed` relies on this; add a comment in the code to make this explicit.

- [ ] **Step 4: Add `store` to `lib.rs`**

```rust
pub mod config;
pub mod store;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag store
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/store.rs crates/rag/src/lib.rs crates/rag/Cargo.toml
git commit -m "feat(rag): add LanceDB store with upsert, delete, search"
```

---

## Chunk 2: Embedder, Extractors, Chunker

### Task 4: Implement `embedder.rs` — Embedder trait + OpenRouter

**Files:**
- Create: `crates/rag/src/embedder.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct StubEmbedder;

    #[async_trait::async_trait]
    impl Embedder for StubEmbedder {
        async fn embed_texts(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0_f32; 3072]).collect())
        }
        async fn describe_image(&self, _image_b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("a test image".to_string())
        }
    }

    #[tokio::test]
    async fn test_stub_embed() {
        let e = StubEmbedder;
        let result = e.embed_texts(&["hello".to_string(), "world".to_string()]).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 3072);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag embedder
```

Expected: FAIL.

- [ ] **Step 3: Implement `embedder.rs`**

```rust
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use crate::config::RagConfig;

#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts. Returns one Vec<f32> per input.
    async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Call vision LLM to describe an image. `image_b64` is base64-encoded bytes.
    /// `mime` is e.g. "image/png".
    async fn describe_image(&self, image_b64: &str, mime: &str) -> Result<String>;
}

pub struct OpenRouterEmbedder {
    client: reqwest::Client,
    config: RagConfig,
}

impl OpenRouterEmbedder {
    pub fn new(config: RagConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: String,
}

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
const BATCH_SIZE: usize = 100;
const MAX_RETRIES: u32 = 3;

async fn post_with_retry(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response> {
    let mut delay_ms = 500u64;
    for attempt in 0..MAX_RETRIES {
        let resp = client
            .post(url)
            .bearer_auth(api_key)
            .header("HTTP-Referer", "https://github.com/super-ragondin")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if status.as_u16() == 429 || status.is_server_error() {
            if attempt + 1 < MAX_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 2;
                continue;
            }
        }
        anyhow::bail!("OpenRouter error {status}: {}", resp.text().await?);
    }
    anyhow::bail!("OpenRouter: exhausted retries")
}

#[async_trait]
impl Embedder for OpenRouterEmbedder {
    async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH_SIZE) {
            let body = EmbedRequest {
                model: &self.config.embed_model,
                input: chunk,
            };
            let resp = post_with_retry(
                &self.client,
                &format!("{OPENROUTER_BASE}/embeddings"),
                &self.config.api_key,
                &body,
            )
            .await?;
            let parsed: EmbedResponse = resp.json().await?;
            all.extend(parsed.data.into_iter().map(|d| d.embedding));
        }
        Ok(all)
    }

    async fn describe_image(&self, image_b64: &str, mime: &str) -> Result<String> {
        let data_url = format!("data:{mime};base64,{image_b64}");
        let body = ChatRequest {
            model: &self.config.vision_model,
            messages: vec![ChatMessage {
                role: "user",
                content: serde_json::json!([
                    {
                        "type": "image_url",
                        "image_url": { "url": data_url }
                    },
                    {
                        "type": "text",
                        "text": "Describe the content of this image in detail, in the language of the text it contains if any. Focus on information that would be useful for search and retrieval."
                    }
                ]),
            }],
            stream: false,
        };
        let resp = post_with_retry(
            &self.client,
            &format!("{OPENROUTER_BASE}/chat/completions"),
            &self.config.api_key,
            &body,
        )
        .await?;
        let parsed: ChatResponse = resp.json().await?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default())
    }
}
```

- [ ] **Step 4: Add to `lib.rs`**

```rust
pub mod config;
pub mod embedder;
pub mod store;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag embedder
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/embedder.rs crates/rag/src/lib.rs crates/rag/Cargo.toml
git commit -m "feat(rag): add Embedder trait and OpenRouterEmbedder"
```

---

### Task 5: Implement `extractor/plaintext.rs`

**Files:**
- Create: `crates/rag/src/extractor/mod.rs`
- Create: `crates/rag/src/extractor/plaintext.rs`

- [ ] **Step 1: Write the failing test**

In `extractor/plaintext.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_utf8() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "Hello, world!\nSecond line.").unwrap();
        let text = extract_plaintext(f.path()).unwrap();
        assert_eq!(text, "Hello, world!\nSecond line.");
    }

    #[test]
    fn test_extract_empty() {
        let f = NamedTempFile::new().unwrap();
        let text = extract_plaintext(f.path()).unwrap();
        assert!(text.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag extractor::plaintext
```

Expected: FAIL.

- [ ] **Step 3: Implement `extractor/plaintext.rs`**

```rust
use std::path::Path;
use anyhow::Result;

pub fn extract_plaintext(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

// tests at bottom
```

- [ ] **Step 4: Create `extractor/image.rs`** (fully implemented — needed by later tasks)

```rust
// extractor/image.rs
use std::path::Path;
use anyhow::Result;

pub fn read_as_base64(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes))
}
```

> **Do NOT create `extractor/mod.rs`, `extractor/pdf.rs`, or `extractor/office.rs` yet.** Those are created once all extractors are fully implemented in Tasks 6 and 7. This avoids committing broken stubs with `todo!()` bodies.

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag extractor::plaintext
```

Expected: PASS.

- [ ] **Step 6: Commit only `plaintext.rs` and `image.rs`**

```bash
git add crates/rag/src/extractor/plaintext.rs crates/rag/src/extractor/image.rs
git commit -m "feat(rag): add plaintext and image extractors"
```

---

### Task 6: Implement `extractor/pdf.rs`

**Files:**
- Modify: `crates/rag/src/extractor/pdf.rs`

The PDF extractor uses `pdf-extract`. If extracted text is < 50 chars (scanned PDF), it falls back to rendering the first page with `pdfium-render` and returning the raw bytes as base64 — the caller (indexer) detects this sentinel and routes to the vision LLM.

> **Setup:** `pdfium-render` requires the pdfium shared library. On Linux:
> ```bash
> # Download from https://github.com/bblanchon/pdfium-binaries/releases
> # Set PDFIUM_DYNAMIC_LIB_PATH env var or put libpdfium.so in LD_LIBRARY_PATH
> ```
> On macOS: `brew install pdfium` (if available) or download from bblanchon/pdfium-binaries.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Download a small test PDF and place at tests/fixtures/sample.pdf
    // A 1-page text PDF: https://www.w3.org/WAI/WCAG21/Techniques/pdf/sample.pdf
    // Place at: crates/rag/tests/fixtures/sample.pdf

    #[test]
    fn test_extract_text_pdf() {
        let path = std::path::Path::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.pdf")
        );
        if !path.exists() {
            eprintln!("Skipping: sample.pdf not present");
            return;
        }
        let result = extract_pdf(path).unwrap();
        assert!(result.len() > 50, "Expected meaningful text from PDF");
    }
}
```

- [ ] **Step 2: Add test fixture**

```bash
mkdir -p crates/rag/tests/fixtures
# Download a small text-based PDF for testing:
curl -o crates/rag/tests/fixtures/sample.pdf \
  "https://www.w3.org/WAI/WCAG21/Techniques/pdf/sample.pdf"
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag extractor::pdf
```

Expected: FAIL (todo!).

- [ ] **Step 4: Implement `extractor/pdf.rs`**

```rust
use std::path::Path;
use anyhow::Result;

const SCANNED_THRESHOLD: usize = 50;

/// Extract text from a PDF file.
/// If extracted text is shorter than SCANNED_THRESHOLD chars, the PDF is likely
/// scanned. Returns empty string in that case — the indexer will route to vision LLM.
pub fn extract_pdf(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.len() < SCANNED_THRESHOLD {
                tracing::debug!(
                    path = %path.display(),
                    chars = trimmed.len(),
                    "PDF appears to be scanned (too little text extracted)"
                );
                return Ok(String::new()); // Caller checks empty → vision fallback
            }
            Ok(trimmed)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "pdf-extract failed");
            Ok(String::new())
        }
    }
}

/// Render the first page of a PDF to a PNG and return as base64.
/// Used by the indexer when `extract_pdf` returns empty.
pub fn render_first_page_as_base64(path: &Path) -> Result<String> {
    use pdfium_render::prelude::*;

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library()?);
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let page = doc.pages().get(0)?;
    let bitmap = page.render_with_config(
        &PdfRenderConfig::new()
            .set_target_width(1200)
            .set_maximum_height(1600),
    )?;
    let img = bitmap.as_image();
    let mut buf = Vec::new();
    // NOTE: `image` 0.25 replaced `ImageOutputFormat` with `ImageFormat`.
    // If `write_to` does not compile, use `img.write_with_encoder(image::codecs::png::PngEncoder::new(&mut std::io::Cursor::new(&mut buf)))?;`
    img.write_to(
        &mut std::io::Cursor::new(&mut buf),
        image::ImageFormat::Png,
    )?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &buf,
    ))
}

// tests at bottom
```

> **Note on `render_first_page_as_base64` testing:** Automated testing of this function requires `pdfium` system library and a scanned PDF fixture. These are not included in CI. The function is exercised manually via the smoke test in Task 11. The test above only covers the text-PDF path (`extract_pdf`).

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag extractor::pdf
```

Expected: PASS (or skipped if fixture missing).

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/extractor/pdf.rs crates/rag/tests/
git commit -m "feat(rag): add PDF extractor with scanned fallback"
```

---

### Task 7: Implement `extractor/office.rs`

**Files:**
- Modify: `crates/rag/src/extractor/office.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Place test fixtures at crates/rag/tests/fixtures/
    // sample.docx — any simple Word document
    // sample.xlsx — any simple spreadsheet

    #[test]
    fn test_extract_docx() {
        let path = std::path::Path::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.docx")
        );
        if !path.exists() { return; }
        let text = extract_docx(path).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn test_extract_xlsx() {
        let path = std::path::Path::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx")
        );
        if !path.exists() { return; }
        let text = extract_xlsx(path).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn test_extract_odt() {
        let path = std::path::Path::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.odt")
        );
        if !path.exists() { return; }
        let text = extract_odt(path).unwrap();
        assert!(!text.is_empty());
    }
}
```

Fixtures needed: `crates/rag/tests/fixtures/sample.docx`, `sample.xlsx`, `sample.odt` — create minimal ones with any office suite or download from a public source. Tests skip gracefully if absent.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-ragondin-rag extractor::office
```

Expected: FAIL (todo!).

- [ ] **Step 3: Implement `extractor/office.rs`**

```rust
use std::io::Read;
use std::path::Path;
use anyhow::Result;
use quick_xml::events::Event;
use quick_xml::Reader;

/// Extract text from a .docx file (ZIP containing word/document.xml).
pub fn extract_docx(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut xml_content = String::new();
    zip.by_name("word/document.xml")?.read_to_string(&mut xml_content)?;
    Ok(xml_text_content(&xml_content))
}

/// Extract text from an .odt file (ZIP containing content.xml).
pub fn extract_odt(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut xml_content = String::new();
    zip.by_name("content.xml")?.read_to_string(&mut xml_content)?;
    Ok(xml_text_content(&xml_content))
}

/// Walk XML events and collect all text content, inserting spaces at element boundaries.
fn xml_text_content(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut parts = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    let s = text.trim().to_string();
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!("XML parse error: {e}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    parts.join(" ")
}

/// Extract text from an .xlsx file using calamine.
pub fn extract_xlsx(path: &Path) -> Result<String> {
    use calamine::{open_workbook_auto, Reader as CalaReader, DataType};
    let mut workbook = open_workbook_auto(path)?;
    let mut lines = Vec::new();
    for sheet_name in workbook.sheet_names().to_vec() {
        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .filter_map(|cell| match cell {
                        DataType::String(s) => Some(s.clone()),
                        DataType::Float(f) => Some(f.to_string()),
                        DataType::Int(i) => Some(i.to_string()),
                        _ => None,
                    })
                    .collect();
                if !cells.is_empty() {
                    lines.push(cells.join("\t"));
                }
            }
        }
    }
    Ok(lines.join("\n"))
}

// tests at bottom
```

- [ ] **Step 4: Create `extractor/mod.rs`** — now that all extractors are implemented, wire them together:

```rust
pub mod image;
pub mod office;
pub mod pdf;
pub mod plaintext;

use std::path::Path;
use anyhow::Result;

/// Extract text content from `path`. Returns `None` if the MIME type is unsupported.
/// Returns `Ok(Some(""))` for PDFs with no extractable text (scanned) — indexer handles fallback.
pub fn extract(path: &Path, mime_type: &str) -> Result<Option<String>> {
    match mime_type {
        "text/plain" | "text/markdown" | "text/csv" | "text/x-markdown" => {
            Ok(Some(plaintext::extract_plaintext(path)?))
        }
        "application/pdf" => Ok(Some(pdf::extract_pdf(path)?)),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Ok(Some(office::extract_docx(path)?))
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Ok(Some(office::extract_xlsx(path)?))
        }
        "application/vnd.oasis.opendocument.text" => {
            Ok(Some(office::extract_odt(path)?))
        }
        // Images handled separately (need async embedder for description)
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" => Ok(None),
        other => {
            tracing::debug!(mime_type = other, path = %path.display(), "Skipping unsupported MIME type");
            Ok(None)
        }
    }
}
```

- [ ] **Step 5: Add extractor module and lib.rs update**

```rust
// crates/rag/src/lib.rs — add:
pub mod extractor;
```

- [ ] **Step 6: Run all extractor tests**

```bash
cargo test -p super-ragondin-rag extractor
```

Expected: PASS (office tests skipped if fixtures missing).

- [ ] **Step 7: Commit**

```bash
git add crates/rag/src/extractor/office.rs crates/rag/src/extractor/mod.rs crates/rag/src/lib.rs
git commit -m "feat(rag): add office extractors (DOCX, ODT, XLSX) and wire extractor dispatch"
```

---

### Task 8: Implement `chunker.rs`

**Files:**
- Create: `crates/rag/src/chunker.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_plaintext_returns_nonempty() {
        let text = "This is the first sentence. This is the second sentence. \
                    And here comes a third one that is a bit longer than the others.";
        let chunks = chunk_text(text, "text/plain").unwrap();
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }

    #[test]
    fn test_chunk_spreadsheet_uses_token_chunker() {
        let rows: Vec<String> = (0..20).map(|i| format!("row{i}\tvalue{i}")).collect();
        let text = rows.join("\n");
        let chunks = chunk_text(&text, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet").unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_image_description_single_chunk() {
        let text = "A photograph of a mountain landscape at sunset.";
        let chunks = chunk_text_single(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag chunker
```

Expected: FAIL.

- [ ] **Step 3: Implement `chunker.rs`**

> **Note:** Verify constructors against https://docs.rs/chonkie/0.1.1/chonkie/ before implementing. The `::new(size, overlap)` signatures below are approximate. If chonkie uses a builder pattern instead, replace e.g. `SentenceChunker::new(512, 50)?` with `SentenceChunker::builder().chunk_size(512).overlap(50).build()?`. The `TextChef` struct is chonkie's primary interface — check if chunkers should be constructed through it. The `.chunk(text)` call and `.text` field on returned chunks are expected to be stable.

```rust
use anyhow::Result;
use chonkie::{SentenceChunker, TokenChunker, RecursiveChunker};

const PROSE_CHUNK_SIZE: usize = 512;
const PROSE_OVERLAP: usize = 50;
const TABLE_CHUNK_SIZE: usize = 256;

/// Split text into chunks appropriate for the given MIME type.
/// Returns raw text strings — no metadata attached here.
pub fn chunk_text(text: &str, mime_type: &str) -> Result<Vec<String>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    match mime_type {
        "text/plain" | "text/markdown" | "text/x-markdown" => chunk_prose_sentence(text),
        "text/csv"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            chunk_tabular(text)
        }
        // PDF, DOCX, ODT — prose with recursive strategy
        _ => chunk_prose_recursive(text),
    }
}

/// For image descriptions and scanned PDF fallbacks — always one chunk.
pub fn chunk_text_single(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

fn chunk_prose_sentence(text: &str) -> Result<Vec<String>> {
    let chunker = SentenceChunker::new(PROSE_CHUNK_SIZE, PROSE_OVERLAP)?;
    let chunks = chunker.chunk(text)?;
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_prose_recursive(text: &str) -> Result<Vec<String>> {
    let chunker = RecursiveChunker::new(PROSE_CHUNK_SIZE, PROSE_OVERLAP)?;
    let chunks = chunker.chunk(text)?;
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_tabular(text: &str) -> Result<Vec<String>> {
    let chunker = TokenChunker::new(TABLE_CHUNK_SIZE, 0)?;
    let chunks = chunker.chunk(text)?;
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

// tests at bottom
```

- [ ] **Step 4: Add to `lib.rs`**

```rust
pub mod chunker;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag chunker
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/chunker.rs crates/rag/src/lib.rs
git commit -m "feat(rag): add chunker with chonkie (sentence, recursive, token strategies)"
```

---

## Chunk 3: Indexer, Searcher, CLI

### Task 9: Implement `indexer.rs`

**Files:**
- Create: `crates/rag/src/indexer.rs`

The indexer's core operation is `reconcile`: compare `TreeStore` synced records against LanceDB, then index new/changed files and delete removed ones. It is async and takes the `Embedder` as a trait object so tests can inject a stub.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::Embedder;
    use crate::store::RagStore;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::io::Write;
    use tempfile::{tempdir, NamedTempFile};

    struct StubEmbedder;

    #[async_trait]
    impl Embedder for StubEmbedder {
        async fn embed_texts(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0_f32; 3072]).collect())
        }
        async fn describe_image(&self, _b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("stub image description".to_string())
        }
    }

    /// Build a minimal SyncedRecord for a file path.
    fn synced_record(rel_path: &str, md5sum: &str) -> super_ragondin_sync::model::SyncedRecord {
        use super_ragondin_sync::model::{LocalFileId, NodeType, RemoteId, SyncedRecord};
        SyncedRecord {
            local_id: LocalFileId::new(1, 1),
            remote_id: RemoteId(rel_path.to_string()),
            rel_path: rel_path.to_string(),
            md5sum: Some(md5sum.to_string()),
            size: None,
            rev: "1".to_string(),
            node_type: NodeType::File,
            local_name: None,
            local_parent_id: None,
            remote_name: None,
            remote_parent_id: None,
        }
    }

    #[tokio::test]
    async fn test_reconcile_indexes_new_file() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        // Write a text file into the sync dir
        let file_path = sync_dir.path().join("notes").join("hello.txt");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "Hello, this is a test note with enough content.").unwrap();

        let rag_store = RagStore::open(db_dir.path()).await.unwrap();
        let embedder = StubEmbedder;
        let records = vec![synced_record("notes/hello.txt", "abc123")];

        reconcile(&records, sync_dir.path(), &rag_store, &embedder).await.unwrap();

        let indexed = rag_store.list_indexed().await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/hello.txt");
    }

    #[tokio::test]
    async fn test_reconcile_removes_deleted_file() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();
        let rag_store = RagStore::open(db_dir.path()).await.unwrap();
        let embedder = StubEmbedder;

        // Seed LanceDB with a chunk for a file that no longer exists in synced records
        rag_store.upsert_chunks(&[crate::store::ChunkRecord {
            id: "old/file.txt:0".to_string(),
            doc_id: "old/file.txt".to_string(),
            mime_type: "text/plain".to_string(),
            mtime: 0,
            chunk_index: 0,
            chunk_text: "old content".to_string(),
            md5sum: "deadbeef".to_string(),
            embedding: vec![0.0_f32; 3072],
        }]).await.unwrap();

        // Reconcile with empty synced records
        reconcile(&[], sync_dir.path(), &rag_store, &embedder).await.unwrap();

        let indexed = rag_store.list_indexed().await.unwrap();
        assert!(indexed.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag indexer
```

Expected: FAIL.

- [ ] **Step 3: Implement `indexer.rs`**

```rust
use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;
use super_ragondin_sync::model::{NodeType, SyncedRecord};
use crate::embedder::Embedder;
use crate::extractor;
use crate::chunker;
use crate::store::{ChunkRecord, RagStore};

/// Reconcile LanceDB index against the current set of synced records.
/// - Files in synced but not indexed (or with different md5sum) → index.
/// - Doc IDs in LanceDB not present in synced → delete.
pub async fn reconcile(
    synced: &[SyncedRecord],
    sync_dir: &Path,
    rag_store: &RagStore,
    embedder: &dyn Embedder,
) -> Result<()> {
    // Build map: doc_id → md5sum for all synced files
    let synced_map: HashMap<&str, &str> = synced
        .iter()
        .filter(|r| r.node_type == NodeType::File)
        .filter_map(|r| r.md5sum.as_deref().map(|md5| (r.rel_path.as_str(), md5)))
        .collect();

    // Build map: doc_id → md5sum for all indexed chunks
    let indexed = rag_store.list_indexed().await?;
    let indexed_map: HashMap<String, String> = indexed
        .into_iter()
        .map(|d| (d.doc_id, d.md5sum))
        .collect();

    // Delete docs that are no longer synced
    for doc_id in indexed_map.keys() {
        if !synced_map.contains_key(doc_id.as_str()) {
            tracing::debug!(doc_id, "Removing deleted file from index");
            rag_store.delete_doc(doc_id).await?;
        }
    }

    // Index new or changed files
    for (rel_path, md5sum) in &synced_map {
        if indexed_map.get(*rel_path).map(String::as_str) == Some(md5sum) {
            continue; // Up to date
        }
        let file_path = sync_dir.join(rel_path);
        if !file_path.exists() {
            tracing::warn!(rel_path, "Synced file not found on disk, skipping");
            continue;
        }
        let mtime = file_path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Detect MIME type
        let mime_type = detect_mime(&file_path);

        // Delete stale chunks before re-indexing
        rag_store.delete_doc(rel_path).await?;

        match index_file(rel_path, &file_path, &mime_type, mtime, md5sum, embedder).await {
            Ok(chunks) => {
                if !chunks.is_empty() {
                    rag_store.upsert_chunks(&chunks).await?;
                    tracing::info!(rel_path, chunks = chunks.len(), "Indexed file");
                }
            }
            Err(e) => {
                tracing::warn!(rel_path, error = %e, "Failed to index file, skipping");
            }
        }
    }

    Ok(())
}

fn detect_mime(path: &Path) -> String {
    infer::get_from_path(path)
        .ok()
        .flatten()
        .map(|t| t.mime_type().to_string())
        // "application/octet-stream" hits the `other =>` arm in extractor::extract → skipped
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

async fn index_file(
    rel_path: &str,
    file_path: &Path,
    mime_type: &str,
    mtime: i64,
    md5sum: &str,
    embedder: &dyn Embedder,
) -> Result<Vec<ChunkRecord>> {
    let texts: Vec<String> = match mime_type {
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {
            // Vision LLM path
            let b64 = crate::extractor::image::read_as_base64(file_path)?;
            let description = embedder.describe_image(&b64, mime_type).await?;
            chunker::chunk_text_single(&description)
        }
        _ => {
            let raw = extractor::extract(file_path, mime_type)?;
            match raw {
                None => return Ok(Vec::new()), // unsupported
                Some(text) if text.is_empty() => {
                    // Scanned PDF fallback
                    if mime_type == "application/pdf" {
                        match crate::extractor::pdf::render_first_page_as_base64(file_path) {
                            Ok(b64) => {
                                let description = embedder.describe_image(&b64, "image/png").await?;
                                chunker::chunk_text_single(&description)
                            }
                            Err(e) => {
                                tracing::warn!(path = %file_path.display(), error = %e, "Could not render scanned PDF");
                                return Ok(Vec::new());
                            }
                        }
                    } else {
                        return Ok(Vec::new());
                    }
                }
                Some(text) => chunker::chunk_text(&text, mime_type)?,
            }
        }
    };

    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let embeddings = embedder.embed_texts(&texts).await?;
    let chunks = texts
        .into_iter()
        .zip(embeddings)
        .enumerate()
        .map(|(i, (text, embedding))| ChunkRecord {
            id: format!("{rel_path}:{i}"),
            doc_id: rel_path.to_string(),
            mime_type: mime_type.to_string(),
            mtime,
            chunk_index: i as u32,
            chunk_text: text,
            md5sum: md5sum.to_string(),
            embedding,
        })
        .collect();

    Ok(chunks)
}

// tests at bottom
```

- [ ] **Step 4: Add to `lib.rs`**

```rust
pub mod indexer;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag indexer
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/indexer.rs crates/rag/src/lib.rs
git commit -m "feat(rag): add indexer with reconcile logic"
```

---

### Task 10: Implement `searcher.rs`

**Files:**
- Create: `crates/rag/src/searcher.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::Embedder;
    use crate::store::{ChunkRecord, RagStore};
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct StubEmbedder;

    #[async_trait]
    impl Embedder for StubEmbedder {
        async fn embed_texts(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0_f32; 3072]).collect())
        }
        async fn describe_image(&self, _b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("stub".to_string())
        }
    }

    #[tokio::test]
    async fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        store.upsert_chunks(&[ChunkRecord {
            id: "notes/a.md:0".to_string(),
            doc_id: "notes/a.md".to_string(),
            mime_type: "text/plain".to_string(),
            mtime: 1_700_000_000,
            chunk_index: 0,
            chunk_text: "Remote work policy details here.".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 3072],
        }]).await.unwrap();

        let embedder = StubEmbedder;
        let results = search("remote work policy", &store, &embedder, 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "notes/a.md");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-ragondin-rag searcher
```

Expected: FAIL.

- [ ] **Step 3: Implement `searcher.rs`**

```rust
use anyhow::Result;
use crate::embedder::Embedder;
use crate::store::{RagStore, SearchResult};

pub async fn search(
    question: &str,
    rag_store: &RagStore,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let embeddings = embedder.embed_texts(&[question.to_string()]).await?;
    let query_vec = embeddings.into_iter().next().unwrap_or_default();
    rag_store.search(&query_vec, limit).await
}

// tests at bottom
```

- [ ] **Step 4: Add to `lib.rs`**

```rust
pub mod searcher;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-ragondin-rag searcher
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/searcher.rs crates/rag/src/lib.rs
git commit -m "feat(rag): add searcher"
```

---

### Task 11: Add `ask` command and sync-loop wiring to CLI

**Files:**
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/cli/Cargo.toml`

- [ ] **Step 1: Add dependencies to `crates/cli/Cargo.toml`**

Add to `[dependencies]`:

```toml
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
futures = "0.3"
reqwest = { version = "0.12", features = ["json", "stream"] }
serde_json = "1"
```

Run `cargo check -p super-ragondin` to confirm it builds before proceeding.

- [ ] **Step 2: Add the `ask` subcommand to `main()`**

In `main()`, add a new arm:

```rust
Some("ask") => cmd_ask(&args[2..]),
```

And in the usage block:
```rust
println!("  ask <question>                   Ask a question about your files");
```

- [ ] **Step 3: Implement `cmd_ask`**

Add this function to `main.rs`:

```rust
fn cmd_ask(args: &[String]) -> Result<()> {
    use super_ragondin_rag::config::RagConfig;
    use super_ragondin_rag::embedder::OpenRouterEmbedder;
    use super_ragondin_rag::store::RagStore;
    use super_ragondin_rag::searcher::search;
    use std::io::Write;

    if args.is_empty() {
        println!("Usage: super-ragondin ask <question>");
        return Ok(());
    }
    let question = args.join(" ");

    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let db_path = config.sync_dir.join(".rag");
    let rag_config = RagConfig::from_env_with_db_path(db_path);

    if rag_config.api_key.is_empty() {
        eprintln!("Error: OPENROUTER_API_KEY environment variable not set");
        std::process::exit(1);
    }

    let embedder = OpenRouterEmbedder::new(rag_config.clone());
    let rt = tokio::runtime::Runtime::new()?;

    let chunks = rt.block_on(async {
        let rag_store = RagStore::open(&rag_config.db_path).await?;
        search(&question, &rag_store, &embedder, 5).await
    }).map_err(|e| Error::Permanent(e.to_string()))?;

    if chunks.is_empty() {
        println!("No relevant documents found.");
        return Ok(());
    }

    // Build context from retrieved chunks
    let context: String = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}\n{}", i + 1, c.doc_id, c.chunk_text))
        .collect::<Vec<_>>()
        .join("\n\n");

    let system_prompt = "You are a helpful assistant. Answer the user's question using only the provided document excerpts. Be concise and accurate. Respond in the same language as the question.";
    let user_prompt = format!(
        "Documents:\n{context}\n\nQuestion: {question}"
    );

    // Stream the chat completion
    let chat_model = rag_config.chat_model.clone();
    let api_key = rag_config.api_key.clone();
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": chat_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ],
        "stream": true
    });

    rt.block_on(async {
        use futures::StreamExt;

        let resp = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(&api_key)
            .header("HTTP-Referer", "https://github.com/super-ragondin")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Permanent(e.to_string()))?;

        let mut stream = resp.bytes_stream();
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let mut done = false;

        while !done {
            match stream.next().await {
                None => break,
                Some(Err(e)) => return Err(Error::Permanent(e.to_string())),
                Some(Ok(bytes)) => {
                    let text = std::str::from_utf8(&bytes).unwrap_or("");
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                done = true;
                                break;
                            }
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(content) = v["choices"][0]["delta"]["content"].as_str() {
                                    write!(out, "{content}").ok();
                                    out.flush().ok();
                                }
                            }
                        }
                    }
                }
            }
        }
        writeln!(out).ok();
        Ok::<_, Error>(())
    })?;

    // Print references
    println!("\nReferences:");
    for (i, chunk) in chunks.iter().enumerate() {
        use std::time::{Duration, UNIX_EPOCH};
        let date = UNIX_EPOCH + Duration::from_secs(chunk.mtime as u64);
        let dt: chrono::DateTime<chrono::Utc> = date.into();
        println!(
            "[{}] {}  ({}, {})",
            i + 1,
            chunk.doc_id,
            chunk.mime_type,
            dt.format("%Y-%m-%d")
        );
        let preview: String = chunk.chunk_text.chars().take(80).collect();
        println!("    \"{preview}...\"");
    }

    Ok(())
}
```

(Dependencies already added in Step 1.)

- [ ] **Step 4: Wire reconcile into the sync loop**

Replace the **entire body** of the existing `run_sync_cycle` function in `main.rs` (keep the same signature). The new body opens a second `TreeStore` handle for the RAG reconciliation — `fjall` allows multiple handles to the same database from the same process.

Replace `run_sync_cycle` in `main.rs` with:

```rust
fn run_sync_cycle(
    rt: &tokio::runtime::Runtime,
    engine: &mut SyncEngine,
    client: &super_ragondin_sync::remote::client::CozyClient,
    config: &mut Config,
) -> Result<()> {
    use super_ragondin_rag::config::RagConfig;
    use super_ragondin_rag::embedder::OpenRouterEmbedder;
    use super_ragondin_rag::indexer::reconcile;
    use super_ragondin_rag::store::RagStore;

    let last_seq =
        rt.block_on(engine.fetch_and_apply_remote_changes(client, config.last_seq.as_deref()))?;
    config.last_seq = Some(last_seq);
    config.save(&config_path())?;

    rt.block_on(engine.run_cycle_async(client))?;

    // RAG reconciliation — only if API key is set
    let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if !api_key.is_empty() {
        let db_path = config.sync_dir.join(".rag");
        let rag_config = RagConfig::from_env_with_db_path(db_path);
        let embedder = OpenRouterEmbedder::new(rag_config.clone());
        let store = super_ragondin_sync::store::TreeStore::open(&config.store_dir())?;
        let synced = store.list_all_synced()?;

        if let Err(e) = rt.block_on(async {
            let rag_store = RagStore::open(&rag_config.db_path).await?;
            reconcile(&synced, &config.sync_dir, &rag_store, &embedder).await
        }) {
            tracing::warn!(error = %e, "RAG reconciliation failed (non-fatal)");
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Build the entire workspace**

```bash
cargo build --workspace
```

Expected: compiles with no errors.

- [ ] **Step 6: Manual smoke test (requires OPENROUTER_API_KEY)**

```bash
# If you have synced files and an API key:
export OPENROUTER_API_KEY=sk-or-...
super-ragondin sync   # one-shot cycle: syncs files and runs RAG reconciliation
super-ragondin ask "What documents do I have about remote work?"
```

Expected: answer followed by references list.

- [ ] **Step 7: Commit**

```bash
git add crates/cli/src/main.rs crates/cli/Cargo.toml
git commit -m "feat(cli): add ask command and RAG reconciliation in sync loop"
```
