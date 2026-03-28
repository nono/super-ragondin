# Super Ragondin

Rust sync client for Cozy Cloud with RAG capabilities, organized as a Cargo workspace.

## Instructions

- Use `docs/specs` directory for specs (and NOT `docs/superpowers/specs`)
- Use `docs/plans` directory for plans (and NOT `docs/superpowers/plans`)
- Use red-green Test-Driven Development
- Add dependencies with the `cargo add` command - try to avoid directly editing the `Cargo.toml` file to add dependencies
- Always run `cargo fmt --all` after editing Rust files
- Always run `cargo clippy --all-features` after editing Rust files and fix any warnings
- Avoid unsafe unwrap() in async tests
- Avoid code duplication between CLI and GUI
- Do not background long-running commands like `npm install` or `cargo test`; run them in the foreground and wait for completion before proceeding.

## Commands

```bash
cargo build                   # Build the project
cargo fmt --all               # Format the code
cargo test -q                 # Run tests
cargo clippy --all-features   # Run linter (pedantic + nursery enabled)
```

## Project Structure

| Crate | Description | Guide |
|---|---|---|
| `crates/sync/` | File synchronization library | [sync](docs/guides/sync.md) |
| `crates/cli/` | CLI binary entry point | [sync](docs/guides/sync.md) |
| `crates/gui/` | Tauri v2 desktop GUI binary | [frontend](docs/guides/frontend.md) |
| `gui-frontend/` | Svelte 5 + Vite frontend | [frontend](docs/guides/frontend.md) |
| `crates/gui-e2e/` | GUI end-to-end tests via WebDriver | [frontend](docs/guides/frontend.md) |
| `crates/rag/` | RAG indexing and search | [rag](docs/guides/rag.md) |
| `crates/codemode/` | JS sandbox + LLM tool-use loop for `ask` | [rag](docs/guides/rag.md) |

## Domain Guides

- [Sync](docs/guides/sync.md) — sync engine, cozy-stack setup, integration tests
- [Proptest & Simulation](docs/guides/proptest.md) — property-based testing, simulator, debugging failures
- [Frontend, GUI & E2E](docs/guides/frontend.md) — Tauri v2, Svelte 5, E2E tests
- [RAG, Codemode & Ask](docs/guides/rag.md) — RAG indexing, JS sandbox, LLM tool-use loop
- [GitHub Actions CI/CD](docs/guides/ci.md) — CI/release workflows, caching tricks

## Cross-Cutting Findings

- Workspace has `unsafe_code = "forbid"` — use `temp-env` crate for env var manipulation in tests instead of `unsafe { std::env::set_var(...) }`
- reqwest requires `rustls-tls` feature instead of default (native-tls) to avoid OpenSSL system dependency
- Tracing `EnvFilter` in a multi-crate workspace must include all relevant crate targets (e.g. `super_ragondin_rag=info`) — a filter like `super_ragondin_sync=info` silently drops logs from other workspace crates

## References

- [Rust guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt)
