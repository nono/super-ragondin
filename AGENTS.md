# Super Ragondin

Rust sync client for Cozy Cloud with RAG capabilities, organized as a Cargo workspace.

## Instructions

- Use `docs/specs` directory for specs (and NOT `docs/superpowers/specs`)
- Use `docs/plans` directory for plans (and NOT `docs/superpowers/plans`)
- Use red-green Test-Driven Development
- Do not commit automatically
- Add dependencies with the `cargo add` command - try to avoid directly editing the `Cargo.toml` file to add dependencies
- Always run `cargo fmt --all` after editing Rust files
- Always run `cargo clippy --all-features` after editing Rust files and fix any warnings
- Avoid unsafe unwrap() in async tests

## Commands

```bash
cargo build                   # Build the project
cargo fmt --all               # Format the code
cargo test -q                 # Run tests
cargo clippy --all-features   # Run linter (pedantic + nursery enabled)
cargo test --test integration_tests -- --ignored  # Run integration tests (requires cozy-stack serve)
```

### Cozy-stack

We can start the cozy-stack server, create an instance (aka a cozy or a user), then register an OAuth client, and get an access token.

```bash
cozy-stack serve
cozy-stack instances add alice.localhost:8080 --passphrase cozy --apps home,drive --email alice@cozy.localhost --public-name Alice
CLIENT_ID=$(cozy-stack instances client-oauth alice.localhost:8080 http://localhost/ desktop-ng github.com/nono/cozy-desktop-experiments)
TOKEN=$(cozy-stack instances token-oauth alice.localhost:8080 $CLIENT_ID "io.cozy.files")
```

Don't forget to clean the instance when you have finished with:

```bash
cozy-stack instances rm --force alice.localhost:8080
```

## Project Structure

Cargo workspace with three crates:

- `crates/cli/` (`super-ragondin`) - CLI binary entry point
- `crates/sync/` (`super-ragondin-sync`) - File synchronization library
  - `src/config.rs` - Configuration (with `src/config/` submodules)
  - `src/error.rs` - Error types
  - `src/model.rs` - Core data types (Node, NodeId, SyncOp)
  - `src/planner.rs` - Sync operation planning
  - `src/local.rs` - Local filesystem watching (with `src/local/` submodules)
  - `src/remote.rs` - Remote Cozy API client (with `src/remote/` submodules)
  - `src/store.rs` - Persistent storage via fjall (with `src/store/` submodules)
  - `src/sync.rs` - Sync engine (with `src/sync/` submodules)
  - `src/simulator.rs` - Property-based testing simulator (with `src/simulator/` submodules)
  - `tests/` - Integration tests
- `crates/rag/` (`super-ragondin-rag`) - RAG indexing and search
  - `src/config.rs` - `RagConfig` — loads env vars, holds model names + LanceDB path
  - `src/store.rs` - `RagStore` — LanceDB wrapper: schema, upsert, delete, vector search
  - `src/embedder.rs` - `Embedder` trait + `OpenRouterEmbedder` — text embeddings + vision descriptions
  - `src/extractor/` - Text extraction by MIME type (plaintext, PDF, DOCX/ODT/XLSX, images)
  - `src/chunker.rs` - `chunk_text(text, mime)` — chonkie-based chunking (sentence/recursive/token)
  - `src/indexer.rs` - `reconcile()` — diffs synced records vs LanceDB, indexes new/changed/deleted files
  - `src/searcher.rs` - `search()` — embeds question, queries LanceDB, returns ranked chunks

## Findings

- Proptest regression files (`*.proptest-regressions`) must be kept and checked into source control — they ensure known failure cases are always re-tested
- reqwest requires `rustls-tls` feature instead of default (native-tls) to avoid OpenSSL system dependency
- Clippy pedantic warns about "CouchDB" needing backticks in doc comments
- LanceDB (0.20+) requires `protoc` (Protocol Buffers compiler) at build time — install via `apt install protobuf-compiler` or set `PROTOC=/path/to/protoc`
- LanceDB 0.20 resolved to arrow 55, not arrow 54 — use `arrow-array = "55"` and `arrow-schema = "55"` in Cargo.toml
- `infer` crate only detects MIME by magic bytes, not file extension — plain text files (`.txt`, `.md`, `.csv`) need an extension-based fallback in `detect_mime()`
- chonkie 0.1.1 feature is `tiktoken` (not `tiktoken-rs`)
- Workspace has `unsafe_code = "forbid"` — use `temp-env` crate for env var manipulation in tests instead of `unsafe { std::env::set_var(...) }`

## RAG Environment Variables

| Variable | Default | Description |
|---|---|---|
| `OPENROUTER_API_KEY` | required | API key for OpenRouter |
| `OPENROUTER_EMBED_MODEL` | `openai/text-embedding-3-large` | Embedding model |
| `OPENROUTER_VISION_MODEL` | `google/gemini-2.0-flash` | Vision/image model |
| `OPENROUTER_CHAT_MODEL` | `mistralai/mistral-small-3.2-24b-instruct` | Chat completion model |

The LanceDB database is stored at `<data_dir>/rag/` (e.g. `~/.local/share/super-ragondin/rag/`), accessible via `config.rag_dir()`.

## References

- [Cozy-stack authentication](https://docs.cozy.io/en/cozy-stack/auth/)
- [Cozy-stack files API](https://docs.cozy.io/en/cozy-stack/files/)
- [io.cozy.files doctype](https://github.com/cozy/cozy-doctypes/blob/master/docs/io.cozy.files.md)
- [Rust guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt)
- [inotify-rs](https://github.com/hannobraun/inotify-rs)
- [fjall - Log-structured, embeddable key-value storage engine in Rust](https://github.com/fjall-rs/fjall)
- [proptest - Hypothesis-like property testing for Rust](https://github.com/proptest-rs/proptest)
- [LanceDB Rust docs](https://docs.rs/lancedb)
- [chonkie - Rust chunking library](https://docs.rs/chonkie)
- [OpenRouter API](https://openrouter.ai/docs)
