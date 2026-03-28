# RAG, Codemode & Ask Assistant

RAG indexing/search, JavaScript sandbox, and LLM tool-use loop for the `ask` command.

## Crate Structure

### `crates/rag/` (`super-ragondin-rag`)

RAG indexing and search:

- `src/config.rs` - `RagConfig` — loads env vars, holds model names + LanceDB path
- `src/store.rs` - `RagStore` — LanceDB wrapper: schema, upsert, delete, vector search; `MetadataFilter`, `DocInfo`, `ChunkInfo`, `DocSort`
- `src/embedder.rs` - `Embedder` trait + `OpenRouterEmbedder` — text embeddings + vision descriptions
- `src/extractor/` - Text extraction by MIME type (plaintext, PDF, DOCX/ODT/XLSX, images)
- `src/chunker.rs` - `chunk_text(text, mime)` — chonkie-based chunking (sentence/recursive/token)
- `src/indexer.rs` - `reconcile()` — diffs synced records vs LanceDB, indexes new/changed/deleted files
- `src/searcher.rs` - `search()` — embeds question, queries LanceDB, returns ranked chunks

### `crates/codemode/` (`super-ragondin-codemode`)

JS sandbox + LLM tool-use loop for `ask` command:

- `src/prompt.rs` - `system_prompt()` — system prompt explaining JS API + examples
- `src/sandbox.rs` - `SandboxContext` thread-local, `jsvalue_to_serde`/`serde_to_jsvalue` helpers, `Sandbox` struct (fresh Boa context per call)
- `src/engine.rs` - `CodeModeEngine` — OpenRouter tool-use loop (max 10 iterations, execute_js tool)
- `src/tools/search.rs` - `search(query, options?)` JS global — vector search via embedder + RagStore
- `src/tools/list_files.rs` - `listFiles(options?)` JS global — metadata-based file discovery
- `src/tools/get_document.rs` - `getDocument(docId)` JS global — all chunks for a document
- `src/tools/sub_agent.rs` - `subAgent(systemPrompt, userPrompt)` JS global — cheap sub-LLM call
- `src/tools/save_file.rs` - `saveFile(path, content, options?)` JS global — write files into sync_dir (utf8/base64 encoding, path traversal prevention)
- `src/tools/list_dirs.rs` - `listDirs(prefix?)` JS global — list immediate subdirectory names at a path within sync_dir
- `src/tools/generate_image.rs` - `generateImage(prompt, options?)` JS global — image generation via OpenRouter, returns base64 string, optionally saves to sync_dir
- `src/tools/path_utils.rs` - `check_relative_path()` — shared path traversal validation used by `save_file` and `generate_image`
- `src/tools/scratchpad.rs` - `remember(key, value)` / `recall(key)` JS globals — in-session key-value scratchpad shared across tool calls within one `ask()` session

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `OPENROUTER_API_KEY` | required | API key for OpenRouter |
| `OPENROUTER_EMBED_MODEL` | `baai/bge-m3` | Embedding model |
| `OPENROUTER_VISION_MODEL` | `google/gemini-2.5-flash` | Vision/image model |
| `OPENROUTER_CHAT_MODEL` | `mistralai/mistral-small-2603` | Chat completion model (main reasoning loop) |
| `OPENROUTER_SUBAGENT_MODEL` | `google/gemini-2.5-flash` | Model for sub-agent summarization calls (cheaper/faster) |
| `OPENROUTER_IMAGE_MODEL` | `google/gemini-3.1-flash-image-preview` | Image generation model |

The LanceDB database is stored at `<data_dir>/rag/` (e.g. `~/.local/share/super-ragondin/rag/`), accessible via `config.rag_dir()`.

## Findings

- LanceDB (0.20+) requires `protoc` (Protocol Buffers compiler) at build time — install via `apt install protobuf-compiler` or set `PROTOC=/path/to/protoc`
- LanceDB 0.20 resolved to arrow 55, not arrow 54 — use `arrow-array = "55"` and `arrow-schema = "55"` in Cargo.toml
- `baai/bge-m3` via OpenRouter produces 1024-dimensional vectors — `EMBED_DIM` must match; mismatches cause `Invalid argument error` on `FixedSizeListArray` construction, and existing LanceDB tables must be dropped/recreated on dimension change
- `infer` crate only detects MIME by magic bytes, not file extension — plain text files (`.txt`, `.md`, `.csv`) need an extension-based fallback in `detect_mime()`
- `detect_mime()` extension fallback must cover common text-based extensions (`.json`, `.yml`, `.rs`, etc.) — `infer` returns `None` for these and they default to `application/octet-stream`, causing the text extractor to skip them
- chonkie 0.1.1 feature is `tiktoken` (not `tiktoken-rs`)
- `OPENROUTER_API_KEY` resolution must check both environment variable and config file (`api_key` field) with consistent precedence across CLI and GUI

## References

- [LanceDB Rust docs](https://docs.rs/lancedb)
- [chonkie - Rust chunking library](https://docs.rs/chonkie)
- [OpenRouter API](https://openrouter.ai/docs)
