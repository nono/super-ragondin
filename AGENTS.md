# Cozy Desktop NG

Rust sync client for Cozy Cloud using the 3-tree model.

## Instructions

- Use red-green Test-Driven Development
- Do not commit automatically
- Add dependencies with the `cargo add` command - try to avoid directly editing the `Cargo.toml` file to add dependencies.

## Commands

```bash
cargo build                   # Build the project
cargo fmt --all               # Format the code
cargo test -q                 # Run tests
cargo clippy --all-features   # Run linter (pedantic + nursery enabled)
```

## Project Structure

- `src/lib.rs` - Library root, module exports
- `src/main.rs` - CLI entry point
- `src/model.rs` - Core data types (Node, NodeId, SyncOp)
- `src/error.rs` - Error types

## Findings

- reqwest requires `rustls-tls` feature instead of default (native-tls) to avoid OpenSSL system dependency
- Clippy pedantic warns about "CouchDB" needing backticks in doc comments

## References

- [Cozy-stack authentication](https://docs.cozy.io/en/cozy-stack/auth/)
- [Cozy-stack files API](https://docs.cozy.io/en/cozy-stack/files/)
- [io.cozy.files doctype](https://github.com/cozy/cozy-doctypes/blob/master/docs/io.cozy.files.md)
- [Rust guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt)
- [inotify-rs](https://github.com/hannobraun/inotify-rs)
- [fjall - Log-structured, embeddable key-value storage engine in Rust](https://github.com/fjall-rs/fjall)
- [proptest - Hypothesis-like property testing for Rust](https://github.com/proptest-rs/proptest)
