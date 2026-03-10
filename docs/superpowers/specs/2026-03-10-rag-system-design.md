# RAG System Design

**Date:** 2026-03-10
**Status:** Approved

## Overview

Add a RAG (Retrieval-Augmented Generation) system to Super Ragondin. Synced files are automatically indexed into an embedded vector database as they are created, updated, or deleted. The user can query the index with a natural-language question via the CLI and receive a generated answer with source references.

## Stack

| Concern | Choice |
|---|---|
| Vector DB | LanceDB (embedded, Rust-native, single directory on disk) |
| Embeddings | OpenRouter → `openai/text-embedding-3-large` (3072d, multilingual) |
| Vision / scanned PDF | OpenRouter → `google/gemini-2.0-flash` (image → text description) |
| Chat LLM | OpenRouter → `mistralai/mistral-small-3.2-24b-instruct` |
| Chunking | `chonkie` crate with tiktoken cl100k_base tokenizer |
| MIME detection | `infer` crate (magic bytes) |

Target languages: French, English, Spanish, German, Italian.
Expected scale: hundreds to a few thousand files.

## Architecture

The RAG system lives in `crates/rag/`. It has no CLI subcommand of its own for indexing — indexing is wired automatically into the CLI sync loop.

```
crates/rag/src/
  lib.rs
  config.rs          # env-var config, model names, LanceDB path
  extractor/
    mod.rs           # dispatch by MIME type
    pdf.rs           # pdf-extract + vision fallback for scanned PDFs
    office.rs        # DOCX/ODT (zip + quick-xml), XLSX (calamine)
    image.rs         # base64 → OpenRouter vision → description text
    plaintext.rs     # UTF-8 read
  chunker.rs         # wraps chonkie, selects strategy by MIME
  embedder.rs        # OpenRouter HTTP client: embeddings + vision
  store.rs           # LanceDB schema, upsert, delete, query
  indexer.rs         # orchestrates extract → chunk → embed → store
  searcher.rs        # embed query → LanceDB search → ranked chunks
```

### Indexing data flow

```
sync_dir files
  → MIME detection (infer)
  → text extraction (per extractor)
  → chunking (chonkie)
  → embedding batch (OpenRouter, up to 100 chunks/request)
  → LanceDB upsert
```

### Query data flow

```
CLI question
  → embed question (OpenRouter)
  → LanceDB vector search (top-5 chunks)
  → build prompt: system context + chunks + question
  → OpenRouter chat completion (streamed)
  → print answer + references
```

### Integration with sync loop

The sync engine's `run_cycle()` returns a `PlanResult` containing the operations that were executed. After each cycle, the CLI passes the executed operations to `indexer::index_ops(ops)`:

- `DownloadNew` / `DownloadUpdate` → extract, chunk, embed, upsert into LanceDB
- `DeleteLocal` → delete all chunks for that `doc_id` from LanceDB

On first run (or after a DB wipe), the indexer bootstraps by comparing all `SyncedRecord` md5sums against what is already in LanceDB, and queues anything missing or changed.

## LanceDB Schema

One table: **`chunks`**

| Column | Type | Notes |
|---|---|---|
| `id` | `string` | `"{rel_path}:{chunk_index}"` — stable, used for upserts |
| `doc_id` | `string` | `rel_path` of the source file |
| `mime_type` | `string` | detected by `infer` crate |
| `mtime` | `int64` | Unix epoch seconds from `SyncedRecord` |
| `chunk_index` | `uint32` | position of this chunk within the document |
| `chunk_text` | `string` | raw text (shown in references) |
| `embedding` | `vector<f32>[3072]` | from `text-embedding-3-large` |

On file update: delete all rows where `doc_id = rel_path`, then insert new chunks.
On file delete: delete all rows where `doc_id = rel_path`.

## Text Extraction

Dispatched by MIME type (detected via magic bytes using the `infer` crate, not file extension).

| MIME type | Strategy | Crate |
|---|---|---|
| `text/plain`, `text/markdown`, `text/csv` | Read as UTF-8 | — |
| `application/pdf` | `pdf-extract`; if no text extracted, fall back to vision LLM | `pdf-extract` |
| `application/vnd.openxmlformats-officedocument.wordprocessingml.document` (.docx) | Unzip + parse `word/document.xml` | `zip` + `quick-xml` |
| `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` (.xlsx) | Parse shared strings + cells | `calamine` |
| `application/vnd.oasis.opendocument.text` (.odt) | Unzip + parse `content.xml` | `zip` + `quick-xml` |
| `image/jpeg`, `image/png`, `image/webp`, `image/gif` | Send as base64 to vision LLM, embed description | `reqwest` |
| Everything else | Skip, log as unsupported | — |

**Scanned PDF fallback:** if `pdf-extract` returns empty or near-empty text (< 50 characters), the PDF is re-routed to the image extraction path (first page rendered as image, sent to vision LLM).

Office formats not listed (`.ppt`, `.pptx`, `.ods`) are skipped in v1.

## Chunking

Tokenizer: `tiktoken-rs` with `cl100k_base` (matches `text-embedding-3-large`).

| Source | Chunker | Chunk size | Overlap |
|---|---|---|---|
| Plain text, Markdown | `SentenceChunker` | 512 tokens | 50 tokens |
| PDF, DOCX, ODT | `RecursiveChunker` (paragraphs → sentences → words) | 512 tokens | 50 tokens |
| XLSX, CSV | `TokenChunker` (rows as text lines) | 256 tokens | 0 |
| Image descriptions | Single chunk — no splitting | — | — |
| Scanned PDF fallback | Single chunk (vision description) | — | — |

## OpenRouter Integration

All OpenRouter calls go through `embedder.rs`.

**Embeddings:**
- Endpoint: OpenRouter OpenAI-compatible API
- Model: `openai/text-embedding-3-large`
- Batched: up to 100 chunk texts per request
- Returns: `Vec<[f32; 3072]>`

**Vision descriptions:**
- Model: `google/gemini-2.0-flash`
- Image sent as base64 data URL in a chat message
- Prompt: `"Describe the content of this image in detail, in the language of the text it contains if any. Focus on information that would be useful for search and retrieval."`
- Returns: description string, then chunked as plain text

**Error handling:** transient HTTP errors (429, 5xx) get 3 retries with exponential backoff. A failed chunk is logged and skipped; it will be re-indexed on the next sync cycle when the file is re-processed.

## CLI Interface

One new top-level subcommand:

```
super-ragondin ask <question>
```

**Flow:**
1. Embed the question (`text-embedding-3-large`)
2. Retrieve top-5 most similar chunks from LanceDB
3. Build prompt with retrieved chunks as context
4. Stream chat completion (`mistral-small-3.2-24b-instruct`)
5. Print answer, then print references

**Example output:**
```
Remote working requires employees to notify their manager at least 48 hours
in advance. The maximum is 3 remote days per week per the 2025 policy update.

References:
[1] notes/policy-update.md  (text/plain, 2025-03-01)
    "The new remote work policy requires employees to..."
[2] docs/hr/policy-2025.pdf  (application/pdf, 2025-02-14)
    "Section 3: Policy on remote working. Starting from Q1..."
```

Metadata filters (mime type, date range) and configurable result count are deferred to a future iteration.

## Configuration

All configuration via environment variables:

| Variable | Default | Description |
|---|---|---|
| `OPENROUTER_API_KEY` | required | API key |
| `OPENROUTER_EMBED_MODEL` | `openai/text-embedding-3-large` | Embedding model |
| `OPENROUTER_VISION_MODEL` | `google/gemini-2.0-flash` | Vision/image model |
| `OPENROUTER_CHAT_MODEL` | `mistralai/mistral-small-3.2-24b-instruct` | Answer generation model |

LanceDB database path defaults to `<sync_dir>/.rag/` alongside the synced files.

## Out of Scope (v1)

- Metadata filters on `ask` (mime type, date range)
- Manual `rag index` command
- `.ppt`, `.pptx`, `.ods` extraction
- Hybrid BM25 + vector search
- Sub-agent / tool-use integration
