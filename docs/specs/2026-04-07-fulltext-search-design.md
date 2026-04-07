# Replace LanceDB vector search with Tantivy full-text search

**Date**: 2026-04-07
**Status**: Approved

## Motivation

The current RAG system uses LanceDB for vector similarity search with OpenRouter embeddings (`baai/bge-m3`, 1024 dimensions). For personal documents on Cozy Cloud, keyword/BM25 search provides better results than semantic vector search — users search for specific terms they remember (company names, dates, document types) rather than abstract concepts. Switching to Tantivy full-text search also eliminates embedding API costs, the `protoc` build dependency, and enables fully offline indexing for text documents.

## Design

### Store layer (`store.rs`)

Replace LanceDB with a Tantivy index. The Tantivy schema:

| Field | Tantivy type | Notes |
|---|---|---|
| `id` | `STRING` (stored, indexed) | `"{doc_id}:{chunk_index}"` |
| `doc_id` | `STRING` (stored, indexed) | Relative path |
| `mime_type` | `STRING` (stored, indexed) | For filtering |
| `mtime` | `I64` (stored, indexed) | Unix seconds |
| `chunk_index` | `U64` (stored, indexed) | Ordering within a doc |
| `chunk_text` | `TEXT` (stored, tokenized) | BM25-searchable |
| `md5sum` | `STRING` (stored, not indexed) | Change detection only |

The `chunk_text` field uses a **lowercase + unicode word-split tokenizer** — no stemming, no stop-word removal. This works for both French and English documents without language detection.

**`RagStore` public API** is preserved: `upsert_chunks()`, `delete_doc()`, `list_indexed()`, `search()`, `list_docs()`, `get_chunks()`, `list_recent()`. Internal implementation changes from LanceDB/Arrow to Tantivy.

**`ChunkRecord`** loses its `embedding: Vec<f32>` field.

**`SearchResult`** is unchanged (doc_id, mime_type, mtime, chunk_text).

**`MetadataFilter`** — `to_where_clause()` is replaced by a method that builds a Tantivy `BooleanQuery` combining term queries on `mime_type`, prefix queries on `doc_id`, and range queries on `mtime`.

**Skipped docs** (files that produce no indexable content) are tracked in a **sidecar JSON file** (`skipped_docs.json`) next to the Tantivy index directory, rather than polluting the search index.

### Chunking (`chunker.rs`)

Increase chunk size from ~512 tokens to ~2000 tokens. The embedding model's context window no longer constrains chunk size. Larger chunks provide more context per search result for the LLM in the `ask` loop. Simplify the chunker configuration accordingly.

### Embedder trait (`embedder.rs`)

Remove `embed_texts()` from the `Embedder` trait. Rename the trait to `VisionDescriber` (or similar) since it now only has `describe_image()`. Rename `OpenRouterEmbedder` to `OpenRouterVision`.

Remove the `OPENROUTER_EMBED_MODEL` environment variable.

### Indexer (`indexer.rs`)

`index_file()` no longer calls `embed_texts()`:

1. Extract text from file (unchanged)
2. Chunk text with larger chunks (~2000 tokens)
3. Build `ChunkRecord` without embeddings
4. Store in Tantivy via `RagStore::upsert_chunks()`

Images and scanned PDFs still call `describe_image()` to get text, then index that text. Indexing text-only files no longer requires an API key.

`reconcile_if_configured()` changes: it should run even without an API key for text files. An API key is only needed when images are present.

### Searcher (`searcher.rs`)

`search()` simplifies: takes a `&str` query, passes it directly to `RagStore::search()` for BM25 matching. The `Embedder` parameter is removed.

### Codemode search tool (`codemode/src/tools/search.rs`)

No longer calls `embedder.embed_texts()`. Passes the query string directly to `RagStore::search()`. The `SandboxContext` no longer needs an `embedder` field for search (still needs vision describer for image indexing in other paths).

### Config (`config.rs`)

- Remove `embed_model` field and `OPENROUTER_EMBED_MODEL` env var
- `db_path` now points to a Tantivy index directory instead of a LanceDB directory
- Rename `rag_dir()` or keep the same path (`<data_dir>/rag/`)

### Dependencies (`Cargo.toml`)

Remove:
- `lancedb`
- `arrow-array`
- `arrow-schema`

Add:
- `tantivy`

Build improvement: `protoc` (Protocol Buffers compiler) is no longer required.

## What does NOT change

- Text extraction pipeline (`extractor/`)
- Image description via vision LLM (`describe_image()`)
- Codemode tools other than `search` (`listFiles`, `getDocument`, `subAgent`, etc.)
- Session management
- CLI and GUI integration points (they call `reconcile_if_configured()` which keeps the same signature)
