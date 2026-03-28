# Sync Guide

File synchronization between local filesystem and Cozy Cloud.

## Crate Structure

### `crates/sync/` (`super-ragondin-sync`)

Core sync library:

- `src/config.rs` - Configuration (with `src/config/` submodules)
- `src/error.rs` - Error types
- `src/ignore.rs` - `IgnoreRules` — gitignore-style file filtering (wraps `ignore` crate)
- `src/logging.rs` - File-based tracing appender setup
- `src/model.rs` - Core data types (Node, NodeId, SyncOp)
- `src/planner.rs` - Sync operation planning
- `src/util.rs` - Shared utilities (e.g. MD5 helpers, serde helpers)
- `src/local.rs` - Local filesystem watching (with `src/local/` submodules)
- `src/remote.rs` - Remote Cozy API client (with `src/remote/` submodules)
- `src/store.rs` - Persistent storage via fjall (with `src/store/` submodules)
- `src/sync.rs` - Sync engine (with `src/sync/` submodules)
- `src/simulator.rs` - Property-based testing simulator (with `src/simulator/` submodules)
- `src/watcher_mux.rs` - `SyncTrigger` enum + `start_watchers()` — shared by CLI and GUI; add watch-loop primitives here, not in the binaries
- `tests/` - Integration tests

### `crates/cli/` (`super-ragondin`)

CLI binary entry point.

## Cozy-stack Setup

Start the cozy-stack server, create an instance, register an OAuth client, and get an access token:

```bash
cozy-stack serve
cozy-stack instances add alice.localhost:8080 --passphrase cozy --apps home,drive --email alice@cozy.localhost --public-name Alice
CLIENT_ID=$(cozy-stack instances client-oauth alice.localhost:8080 http://localhost/ desktop-ng github.com/nono/cozy-desktop-experiments)
TOKEN=$(cozy-stack instances token-oauth alice.localhost:8080 $CLIENT_ID "io.cozy.files")
```

Clean the instance when finished:

```bash
cozy-stack instances rm --force alice.localhost:8080
```

## Integration Tests

```bash
cargo test --test integration_tests -- --ignored  # requires cozy-stack serve
```

## Findings

- Clippy pedantic warns about "CouchDB" needing backticks in doc comments
- Real-time sync requires coalescing `inotify` + WebSocket events via a `SyncTrigger` enum into a single channel, with `CloseWrite` tracking and a 30s safety timeout for stale pending writes
- GUI sync loop runs on a dedicated OS thread with its own Tokio runtime due to HRTB lifetime constraints in `SyncEngine::run_cycle_async`

## References

- [Cozy-stack authentication](https://docs.cozy.io/en/cozy-stack/auth/)
- [Cozy-stack files API](https://docs.cozy.io/en/cozy-stack/files/)
- [io.cozy.files doctype](https://github.com/cozy/cozy-doctypes/blob/master/docs/io.cozy.files.md)
- [inotify-rs](https://github.com/hannobraun/inotify-rs)
- [fjall - Log-structured, embeddable key-value storage engine in Rust](https://github.com/fjall-rs/fjall)
