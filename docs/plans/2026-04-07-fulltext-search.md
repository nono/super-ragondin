# Fulltext Search (Tantivy) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace LanceDB vector search with Tantivy BM25 full-text search in the RAG crate, removing all embedding dependencies.

**Architecture:** The `RagStore` is rewritten to use a Tantivy index instead of LanceDB. The `Embedder` trait drops its `embed_texts()` method. The indexer no longer computes embeddings; the searcher passes query strings directly to Tantivy. Chunk sizes increase from ~512 to ~2000 tokens.

**Tech Stack:** Rust, Tantivy, chonkie (with larger chunk config)

**Spec:** `docs/specs/2026-04-07-fulltext-search-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/rag/Cargo.toml` | Modify | Remove lancedb/arrow deps, add tantivy |
| `crates/rag/src/store.rs` | Rewrite | Tantivy index: schema, upsert, delete, search, list |
| `crates/rag/src/embedder.rs` | Modify | Remove `embed_texts()` from trait, rename trait to `VisionDescriber` |
| `crates/rag/src/config.rs` | Modify | Remove `embed_model` field |
| `crates/rag/src/indexer.rs` | Modify | Remove embedding step from `index_file()` |
| `crates/rag/src/searcher.rs` | Modify | Remove `Embedder` param, pass query string directly |
| `crates/rag/src/chunker.rs` | Modify | Increase chunk sizes from 512 to 2000 |
| `crates/rag/src/lib.rs` | Modify | Update re-exports if trait name changes |
| `crates/codemode/src/sandbox.rs` | Modify | Update `SandboxContext` to remove embedder field |
| `crates/codemode/src/tools/search.rs` | Modify | Remove embed call, pass query directly |

---

### Task 1: Replace LanceDB with Tantivy in Cargo.toml

**Files:**
- Modify: `crates/rag/Cargo.toml`

- [ ] **Step 1: Remove LanceDB and Arrow dependencies, add Tantivy**

In `crates/rag/Cargo.toml`, replace the `# Vector DB` section:

```toml
# Remove these three lines:
# lancedb = "0.26"
# arrow-array = "57"
# arrow-schema = "57"

# Add:
# Full-text search
tantivy = "0.22"
```

Run:
```bash
cd crates/rag
cargo rm lancedb arrow-array arrow-schema
cargo add tantivy@0.22
```

- [ ] **Step 2: Verify it compiles (it won't yet — just check deps resolve)**

Run: `cargo check -p super-ragondin-rag 2>&1 | head -20`
Expected: Dependency resolution succeeds, but compilation fails (store.rs still references lancedb). That's fine — we'll fix it in the next task.

- [ ] **Step 3: Commit**

```bash
git add crates/rag/Cargo.toml Cargo.lock
git commit -m "chore(rag): replace lancedb/arrow deps with tantivy"
```

---

### Task 2: Rewrite RagStore with Tantivy

This is the largest task. We rewrite `store.rs` to use a Tantivy index.

**Files:**
- Rewrite: `crates/rag/src/store.rs`

- [ ] **Step 1: Write the failing test for `RagStore::open` and `upsert_chunks`**

Replace the entire contents of `crates/rag/src/store.rs` with the new Tantivy-based implementation. Start with the struct definitions, schema, `open()`, `upsert_chunks()`, and the first test.

The `ChunkRecord` struct no longer has an `embedding` field:

```rust
pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_index: u32,
    pub chunk_text: String,
    pub md5sum: String,
}
```

The `RagStore` uses Tantivy `Index` + `IndexReader`:

```rust
use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

const HEAP_SIZE: usize = 50_000_000; // 50 MB writer heap

pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_index: u32,
    pub chunk_text: String,
    pub md5sum: String,
}

pub struct IndexedDoc {
    pub doc_id: String,
    pub md5sum: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_text: String,
}

pub enum DocSort {
    Recent,
    Oldest,
}

#[derive(Debug, Clone)]
pub struct DocInfo {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
}

#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub chunk_index: u32,
    pub chunk_text: String,
}

pub struct MetadataFilter {
    pub mime_type: Option<String>,
    pub path_prefix: Option<String>,
    pub after: Option<i64>,
    pub before: Option<i64>,
}

struct Fields {
    id: Field,
    doc_id: Field,
    mime_type: Field,
    mtime: Field,
    chunk_index: Field,
    chunk_text: Field,
    md5sum: Field,
}

pub struct RagStore {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    skipped_path: std::path::PathBuf,
}
```

The `open()` method creates or opens a Tantivy index at the given path:

```rust
impl RagStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(db_path)?;

        let mut schema_builder = Schema::builder();
        let id = schema_builder.add_text_field("id", STRING | STORED);
        let doc_id = schema_builder.add_text_field("doc_id", STRING | STORED);
        let mime_type = schema_builder.add_text_field("mime_type", STRING | STORED);
        let mtime = schema_builder.add_i64_field("mtime", INDEXED | STORED);
        let chunk_index = schema_builder.add_u64_field("chunk_index", INDEXED | STORED);
        let chunk_text = schema_builder.add_text_field(
            "chunk_text",
            TextOptions::default()
                .set_indexing_options(
                    TextFieldIndexing::default()
                        .set_tokenizer("default")
                        .set_index_option(IndexRecordOption::WithFreqsAndPositions),
                )
                .set_stored(),
        );
        let md5sum = schema_builder.add_text_field("md5sum", STORED);
        let schema = schema_builder.build();

        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(db_path)?,
            schema,
        )?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let fields = Fields {
            id,
            doc_id,
            mime_type,
            mtime,
            chunk_index,
            chunk_text,
            md5sum,
        };

        let skipped_path = db_path.join("skipped_docs.json");

        Ok(Self {
            index,
            reader,
            fields,
            skipped_path,
        })
    }
}
```

Note: `open()` is no longer `async` — Tantivy is synchronous. All public methods on `RagStore` become synchronous (not `async`). This will require updating callers (indexer, searcher, codemode) to drop `.await` calls.

- [ ] **Step 2: Implement `upsert_chunks()`**

```rust
impl RagStore {
    pub fn upsert_chunks(&self, chunks: &[ChunkRecord]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)?;
        for chunk in chunks {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.id, &chunk.id);
            doc.add_text(self.fields.doc_id, &chunk.doc_id);
            doc.add_text(self.fields.mime_type, &chunk.mime_type);
            doc.add_i64(self.fields.mtime, chunk.mtime);
            doc.add_u64(self.fields.chunk_index, u64::from(chunk.chunk_index));
            doc.add_text(self.fields.chunk_text, &chunk.chunk_text);
            doc.add_text(self.fields.md5sum, &chunk.md5sum);
            writer.add_document(doc)?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }
}
```

- [ ] **Step 3: Implement `delete_doc()`**

```rust
impl RagStore {
    pub fn delete_doc(&self, doc_id: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(HEAP_SIZE)?;
        let term = tantivy::Term::from_field_text(self.fields.doc_id, doc_id);
        writer.delete_term(term);
        writer.commit()?;
        self.reader.reload()?;
        // Also remove from skipped docs
        self.remove_skipped(doc_id)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Implement skipped docs (sidecar JSON)**

```rust
impl RagStore {
    pub fn upsert_skipped(&self, doc_id: &str, md5sum: &str) -> Result<()> {
        let mut map = self.load_skipped()?;
        map.insert(doc_id.to_string(), md5sum.to_string());
        self.save_skipped(&map)
    }

    fn remove_skipped(&self, doc_id: &str) -> Result<()> {
        let mut map = self.load_skipped()?;
        if map.remove(doc_id).is_some() {
            self.save_skipped(&map)?;
        }
        Ok(())
    }

    fn load_skipped(&self) -> Result<HashMap<String, String>> {
        if self.skipped_path.exists() {
            let data = std::fs::read_to_string(&self.skipped_path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(HashMap::new())
        }
    }

    fn save_skipped(&self, map: &HashMap<String, String>) -> Result<()> {
        let data = serde_json::to_string_pretty(map)?;
        std::fs::write(&self.skipped_path, data)?;
        Ok(())
    }
}
```

- [ ] **Step 5: Implement `list_indexed()`**

Collect unique `doc_id` + `md5sum` from all Tantivy docs, plus skipped docs:

```rust
impl RagStore {
    pub fn list_indexed(&self) -> Result<Vec<IndexedDoc>> {
        let searcher = self.reader.searcher();
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for segment_reader in searcher.segment_readers() {
            let store_reader = segment_reader.get_store_reader(1)?;
            for doc_id_ord in 0..segment_reader.num_docs() {
                let doc: TantivyDocument = store_reader.get(doc_id_ord)?;
                let doc_id = doc
                    .get_first(self.fields.doc_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if seen.insert(doc_id.clone()) {
                    let md5sum = doc
                        .get_first(self.fields.md5sum)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    result.push(IndexedDoc { doc_id, md5sum });
                }
            }
        }

        // Merge in skipped docs
        let skipped = self.load_skipped()?;
        for (doc_id, md5sum) in skipped {
            if seen.insert(doc_id.clone()) {
                result.push(IndexedDoc { doc_id, md5sum });
            }
        }

        Ok(result)
    }
}
```

- [ ] **Step 6: Implement `search()`**

BM25 search on `chunk_text`, with optional metadata filtering:

```rust
impl MetadataFilter {
    pub fn to_tantivy_query(&self, fields: &Fields) -> Option<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        if let Some(mime) = &self.mime_type {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    tantivy::Term::from_field_text(fields.mime_type, mime),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        if let Some(prefix) = &self.path_prefix {
            let prefix_with_slash = if prefix.ends_with('/') {
                prefix.clone()
            } else {
                format!("{prefix}/")
            };
            // For path prefix matching, we use a regex or a set of term queries.
            // Since doc_id is STRING (not tokenized), we can use a PhrasePrefixQuery
            // workaround: collect all matching doc_ids. But simpler: use a regex query.
            // Tantivy doesn't have LIKE, so we use a regex on the doc_id field.
            let escaped = regex_syntax::escape(&prefix_with_slash);
            clauses.push((
                Occur::Must,
                Box::new(tantivy::query::RegexQuery::from_pattern(
                    &format!("{escaped}.*"),
                    fields.doc_id,
                )?),
            ));
        }
        if let Some(after) = self.after {
            clauses.push((
                Occur::Must,
                Box::new(RangeQuery::new_i64_bounds(
                    fields.mtime,
                    std::ops::Bound::Excluded(after),
                    std::ops::Bound::Unbounded,
                )),
            ));
        }
        if let Some(before) = self.before {
            clauses.push((
                Occur::Must,
                Box::new(RangeQuery::new_i64_bounds(
                    fields.mtime,
                    std::ops::Bound::Unbounded,
                    std::ops::Bound::Excluded(before),
                )),
            ));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(Box::new(BooleanQuery::new(clauses)))
        }
    }
}

impl RagStore {
    pub fn search(
        &self,
        query_str: &str,
        limit: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.chunk_text]);
        let text_query = query_parser.parse_query(query_str)?;

        let final_query: Box<dyn Query> = if let Some(f) = filter
            && let Some(filter_query) = f.to_tantivy_query(&self.fields)
        {
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, text_query),
                (Occur::Must, filter_query),
            ]))
        } else {
            text_query
        };

        let top_docs = searcher.search(&final_query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            results.push(SearchResult {
                doc_id: doc
                    .get_first(self.fields.doc_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                mime_type: doc
                    .get_first(self.fields.mime_type)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                mtime: doc
                    .get_first(self.fields.mtime)
                    .and_then(|v| v.as_i64())
                    .unwrap_or_default(),
                chunk_text: doc
                    .get_first(self.fields.chunk_text)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        Ok(results)
    }
}
```

- [ ] **Step 7: Implement `list_docs()`**

```rust
impl RagStore {
    pub fn list_docs(
        &self,
        filter: Option<&MetadataFilter>,
        sort: DocSort,
        limit: Option<usize>,
    ) -> Result<Vec<DocInfo>> {
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = if let Some(f) = filter
            && let Some(fq) = f.to_tantivy_query(&self.fields)
        {
            fq
        } else {
            Box::new(tantivy::query::AllQuery)
        };

        let all_docs = searcher.search(&query, &TopDocs::with_limit(100_000))?;

        let mut map: HashMap<String, DocInfo> = HashMap::new();
        for (_score, doc_address) in all_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let doc_id = doc
                .get_first(self.fields.doc_id)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let mtime = doc
                .get_first(self.fields.mtime)
                .and_then(|v| v.as_i64())
                .unwrap_or_default();
            map.entry(doc_id.clone())
                .and_modify(|e| {
                    if mtime > e.mtime {
                        e.mtime = mtime;
                    }
                })
                .or_insert_with(|| DocInfo {
                    doc_id,
                    mime_type: doc
                        .get_first(self.fields.mime_type)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    mtime,
                });
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
}
```

- [ ] **Step 8: Implement `list_recent()` and `get_chunks()`**

```rust
impl RagStore {
    pub fn list_recent(&self, since: std::time::SystemTime) -> Result<Vec<String>> {
        let since_secs = since
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .cast_signed();
        let filter = MetadataFilter {
            mime_type: None,
            path_prefix: None,
            after: Some(since_secs),
            before: None,
        };
        let docs = self.list_docs(Some(&filter), DocSort::Recent, Some(20))?;
        Ok(docs.into_iter().map(|d| d.doc_id).collect())
    }

    pub fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkInfo>> {
        let searcher = self.reader.searcher();
        let query = TermQuery::new(
            tantivy::Term::from_field_text(self.fields.doc_id, doc_id),
            IndexRecordOption::Basic,
        );
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10_000))?;

        let mut chunks = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let chunk_index = doc
                .get_first(self.fields.chunk_index)
                .and_then(|v| v.as_u64())
                .unwrap_or_default();
            let chunk_text = doc
                .get_first(self.fields.chunk_text)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            chunks.push(ChunkInfo {
                chunk_index: u32::try_from(chunk_index).unwrap_or(u32::MAX),
                chunk_text,
            });
        }
        chunks.sort_by_key(|c| c.chunk_index);
        Ok(chunks)
    }
}
```

- [ ] **Step 9: Write tests**

Port all existing tests from the LanceDB version. Key changes: remove `async` from tests, remove `embedding` from `make_chunk()`, use synchronous method calls.

```rust
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
        }
    }

    #[test]
    fn test_upsert_and_list_indexed() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        let chunk = make_chunk("notes/test.md", 0, "hello world");
        store.upsert_chunks(&[chunk]).unwrap();

        let indexed = store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/test.md");
        assert_eq!(indexed[0].md5sum, "abc123");
    }

    #[test]
    fn test_delete_by_doc() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/a.md", 0, "aaa")])
            .unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/b.md", 0, "bbb")])
            .unwrap();
        store.delete_doc("notes/a.md").unwrap();

        let indexed = store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/b.md");
    }

    #[test]
    fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/a.md", 0, "remote work policy details")])
            .unwrap();

        let results = store.search("remote work policy", 5, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "notes/a.md");
    }

    #[test]
    fn test_search_with_mime_filter() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();

        let mut pdf_chunk = make_chunk("docs/report.pdf", 0, "quarterly results");
        pdf_chunk.mime_type = "application/pdf".to_string();
        let txt_chunk = make_chunk("notes/a.md", 0, "quarterly results");

        store.upsert_chunks(&[pdf_chunk, txt_chunk]).unwrap();

        let filter = MetadataFilter {
            mime_type: Some("application/pdf".to_string()),
            path_prefix: None,
            after: None,
            before: None,
        };
        let results = store.search("quarterly", 5, Some(&filter)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "docs/report.pdf");
    }

    #[test]
    fn test_get_chunks_ordered() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[
                make_chunk("notes/a.md", 1, "second chunk"),
                make_chunk("notes/a.md", 0, "first chunk"),
            ])
            .unwrap();

        let chunks = store.get_chunks("notes/a.md").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].chunk_text, "first chunk");
        assert_eq!(chunks[1].chunk_index, 1);
        assert_eq!(chunks[1].chunk_text, "second chunk");
    }
}
```

- [ ] **Step 10: Run tests and fix**

Run: `cargo test -p super-ragondin-rag --lib store -- --nocapture`
Expected: All tests pass.

- [ ] **Step 11: Format and lint**

Run:
```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```
Fix any warnings.

- [ ] **Step 12: Commit**

```bash
git add crates/rag/src/store.rs
git commit -m "feat(rag): rewrite RagStore with Tantivy full-text search"
```

---

### Task 3: Simplify the Embedder trait

**Files:**
- Modify: `crates/rag/src/embedder.rs`
- Modify: `crates/rag/src/lib.rs`

- [ ] **Step 1: Remove `embed_texts()` from the trait, rename to `VisionDescriber`**

In `crates/rag/src/embedder.rs`:

1. Rename the trait from `Embedder` to `VisionDescriber`
2. Remove the `embed_texts()` method from the trait
3. Rename `OpenRouterEmbedder` to `OpenRouterVision`
4. Remove the `EmbedRequest`, `EmbedResponse`, `EmbedData` structs
5. Remove the `BATCH_SIZE` const
6. Remove the `embed_texts()` implementation from `OpenRouterVision`
7. Keep `describe_image()` and all its supporting types (`ChatRequest`, etc.)
8. Update tests

The trait becomes:

```rust
#[async_trait]
pub trait VisionDescriber: Send + Sync {
    async fn describe_image(&self, image_b64: &str, mime: &str) -> Result<String>;
}
```

`OpenRouterVision` struct keeps the same `client` + `config` fields, but only implements `describe_image()`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-ragondin-rag --lib embedder`
Expected: PASS

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/embedder.rs
git commit -m "refactor(rag): rename Embedder to VisionDescriber, remove embed_texts()"
```

---

### Task 4: Remove `embed_model` from config

**Files:**
- Modify: `crates/rag/src/config.rs`

- [ ] **Step 1: Remove `embed_model` field and its env var**

In `crates/rag/src/config.rs`:

1. Remove the `embed_model: String` field from `RagConfig`
2. Remove `embed_model` from the `Debug` impl
3. Remove the `OPENROUTER_EMBED_MODEL` line from `from_env_with_db_path()`
4. Update tests: remove `OPENROUTER_EMBED_MODEL` from `with_vars_unset` lists, remove the `test_config_from_env` assertion on `embed_model`, remove any test that only tests embed_model

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-ragondin-rag --lib config`
Expected: PASS

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/config.rs
git commit -m "refactor(rag): remove embed_model from RagConfig"
```

---

### Task 5: Update indexer — remove embedding step

**Files:**
- Modify: `crates/rag/src/indexer.rs`

- [ ] **Step 1: Update `index_file()` to stop computing embeddings**

In `crates/rag/src/indexer.rs`:

1. Change `embedder: &dyn Embedder` to `describer: &dyn VisionDescriber` in function signatures
2. In `index_file()`, remove the `embedder.embed_texts(&texts).await?` call and the zip with embeddings. Build `ChunkRecord` without the `embedding` field.
3. In `reconcile()`, rename the parameter from `embedder` to `describer`
4. In `reconcile_if_configured()`, rename `OpenRouterEmbedder` to `OpenRouterVision`
5. In `index_file()` for images, change `embedder.describe_image()` to `describer.describe_image()`
6. All `RagStore` calls become synchronous (remove `.await`)
7. Update `reconcile_if_configured()`: it should now run even without an API key. When no API key is present, pass a stub/no-op `VisionDescriber` that returns an error for `describe_image()` — text files will still be indexed normally, only images will be skipped. Remove the early `let Some(api_key) = api_key else { return };` guard.

The new `index_file()` body for the text path:

```rust
// After extracting and chunking text:
let chunks = texts
    .into_iter()
    .enumerate()
    .map(|(i, text)| ChunkRecord {
        id: format!("{rel_path}:{i}"),
        doc_id: rel_path.to_string(),
        mime_type: mime_type.to_string(),
        mtime,
        chunk_index: u32::try_from(i).expect("chunk index fits u32"),
        chunk_text: text,
        md5sum: md5sum.to_string(),
    })
    .collect();
```

8. Update `StubEmbedder` in tests to `StubDescriber` implementing `VisionDescriber`
9. Remove all `embedding: vec![0.0_f32; 1024]` from test code
10. Since `RagStore` methods are now sync but `describe_image()` is still async, `reconcile()` stays `async`

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-ragondin-rag --lib indexer`
Expected: PASS

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/indexer.rs
git commit -m "refactor(rag): remove embedding step from indexer"
```

---

### Task 6: Simplify searcher

**Files:**
- Modify: `crates/rag/src/searcher.rs`

- [ ] **Step 1: Remove `Embedder` param, pass query string directly**

Replace the contents of `searcher.rs`:

```rust
use crate::store::{MetadataFilter, RagStore, SearchResult};
use anyhow::Result;

pub fn search(
    question: &str,
    rag_store: &RagStore,
    limit: usize,
    filter: Option<&MetadataFilter>,
) -> Result<Vec<SearchResult>> {
    rag_store.search(question, limit, filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ChunkRecord, RagStore};
    use tempfile::tempdir;

    #[test]
    fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[ChunkRecord {
                id: "notes/a.md:0".to_string(),
                doc_id: "notes/a.md".to_string(),
                mime_type: "text/plain".to_string(),
                mtime: 1_700_000_000,
                chunk_index: 0,
                chunk_text: "Remote work policy details here.".to_string(),
                md5sum: "abc".to_string(),
            }])
            .unwrap();

        let results = search("remote work policy", &store, 5, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "notes/a.md");
    }
}
```

Note: `search()` is no longer `async` and no longer takes an `embedder` parameter.

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-ragondin-rag --lib searcher`
Expected: PASS

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/searcher.rs
git commit -m "refactor(rag): simplify searcher to pass query string directly"
```

---

### Task 7: Increase chunk sizes

**Files:**
- Modify: `crates/rag/src/chunker.rs`

- [ ] **Step 1: Change chunk size constants**

In `crates/rag/src/chunker.rs`:

```rust
// Old values:
// const PROSE_CHUNK_SIZE: usize = 512;
// const TABLE_CHUNK_SIZE: usize = 256;

// New values:
const PROSE_CHUNK_SIZE: usize = 2000;
const PROSE_OVERLAP: usize = 200;
const TABLE_CHUNK_SIZE: usize = 1000;
```

Also increase `SENTENCE_MAX_PER_CHUNK` to accommodate larger chunks:

```rust
const SENTENCE_MAX_PER_CHUNK: usize = 48;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-ragondin-rag --lib chunker`
Expected: PASS

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-rag
```

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/chunker.rs
git commit -m "feat(rag): increase chunk sizes to ~2000 tokens for BM25"
```

---

### Task 8: Update codemode search tool

**Files:**
- Modify: `crates/codemode/src/tools/search.rs`
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Update `SandboxContext` — remove `embedder` field**

In `crates/codemode/src/sandbox.rs`:

1. Remove `use super_ragondin_rag::embedder::OpenRouterEmbedder` from imports (keep `config::RagConfig` and `store::RagStore`)
2. Remove `pub embedder: Arc<OpenRouterEmbedder>` from `SandboxContext`
3. In `Sandbox::execute()`, remove `let embedder = Arc::new(OpenRouterEmbedder::new(self.config.clone()));` and remove `embedder,` from the `SandboxContext` construction

- [ ] **Step 2: Update search tool — remove embed call**

In `crates/codemode/src/tools/search.rs`:

1. Remove `use super_ragondin_rag::embedder::Embedder;`
2. In `search_fn()`, replace the embedder call with a direct store search:

```rust
let results = SANDBOX_CTX.with(|cell| {
    let borrow = cell.borrow();
    let sandbox = borrow.as_ref().ok_or_else(|| {
        JsNativeError::error().with_message("sandbox context not initialized")
    })?;
    let store = std::sync::Arc::clone(&sandbox.store);
    let filter_opt = if has_filter { Some(filter) } else { None };
    store
        .search(&query, limit, filter_opt.as_ref())
        .map_err(|e| JsNativeError::error().with_message(e.to_string()).into())
})?;
```

Note: `store.search()` is now synchronous — no `block_on` needed, no `handle` needed for this tool.

- [ ] **Step 3: Run tests**

Run: `cargo test -p super-ragondin-codemode`
Expected: PASS (or fix any remaining embedder references in other codemode files)

- [ ] **Step 4: Check for remaining `embedder` or `embed_texts` references in codemode**

Run:
```bash
grep -rn "embedder\|embed_texts\|OpenRouterEmbedder" crates/codemode/src/
```
Expected: No results. If any remain, fix them.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/sandbox.rs crates/codemode/src/tools/search.rs
git commit -m "refactor(codemode): remove embedder from search tool, use direct BM25 query"
```

---

### Task 9: Full build and test verification

- [ ] **Step 1: Build the entire workspace**

Run: `cargo build`
Expected: Clean build, no errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test -q`
Expected: All tests pass.

- [ ] **Step 3: Run clippy on entire workspace**

Run: `cargo clippy --all-features`
Expected: No warnings.

- [ ] **Step 4: Update documentation**

Update `docs/guides/rag.md`:
- Replace LanceDB references with Tantivy
- Remove `OPENROUTER_EMBED_MODEL` from env var table
- Update the findings section (remove protoc requirement, LanceDB arrow version notes, embed dimension notes)
- Update `store.rs` description: "Tantivy wrapper: schema, upsert, delete, full-text search"
- Update `embedder.rs` description: "VisionDescriber trait + OpenRouterVision — image descriptions"
- Update `searcher.rs` description: "search() — passes query to Tantivy BM25, returns ranked chunks"

- [ ] **Step 5: Commit**

```bash
git add docs/guides/rag.md
git commit -m "docs(rag): update guide for Tantivy full-text search"
```
