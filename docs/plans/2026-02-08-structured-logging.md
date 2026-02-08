# Structured JSONL Logging Implementation Plan

**Goal:** Replace the current basic tracing setup with a dual-output structured logging system: human-readable on stderr + JSONL to daily-rotated log files, with comprehensive log coverage across all library modules.

**Architecture:** Two `tracing-subscriber` layers on a single registry — a human-friendly `fmt` layer writing to stderr, and a JSON layer writing to a daily-rotated file via `tracing-appender`. Log files go to `$XDG_STATE_HOME/cozy-desktop/` (per XDG spec, state data like logs). Structured fields use emojis to visually categorize log events. The `#[instrument]` attribute is used on key functions to automatically capture arguments as span fields.

**Tech Stack:** `tracing` 0.1, `tracing-subscriber` 0.3 (features: `env-filter`, `json`), `tracing-appender` 0.2

---

## Emoji Convention

Emojis are placed at the start of every log message string, e.g. `tracing::info!(count = 42, "🔍 Scan complete")`. This makes them visible both in the human-readable stderr output and in the JSONL `message` field.

| Category | Emoji | Used for |
|----------|-------|----------|
| Scan | 🔍 | Local filesystem scanning |
| Download | 📥 | Downloading from remote |
| Upload | 📤 | Uploading to remote |
| Create | 📁 | Creating directory (local or remote) |
| Delete | 🗑️ | Deleting file/directory |
| Move | 🔀 | Moving/renaming |
| Conflict | ⚠️ | Sync conflicts |
| Sync | 🔄 | Sync cycle start/end |
| Plan | 📋 | Planning phase |
| Store | 💾 | Store operations |
| Watch | 👁️ | Filesystem watcher events |
| Auth | 🔑 | Authentication |
| Network | 🌐 | HTTP requests |
| Config | ⚙️ | Configuration |
| Error | ❌ | Errors |
| Skip | ⏭️ | Skipped items (symlinks, TOCTOU) |

---

### Task 1: Add `tracing-appender` Dependency

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add the dependency and enable the `json` feature**

```bash
cargo add tracing-appender@0.2
```

Then edit `Cargo.toml` to add `"json"` to `tracing-subscriber` features:

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully

**Step 3: Commit**

```
feat: add tracing-appender and json feature for structured logging
```

---

### Task 2: Create the Logging Module

**Files:**
- Create: `src/logging.rs`
- Modify: `src/lib.rs` (add `pub mod logging;`)

**Step 1: Write a test for `log_dir()`**

In `src/logging.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_uses_xdg_state_home() {
        std::env::set_var("XDG_STATE_HOME", "/tmp/test-xdg-state");
        let dir = log_dir();
        assert_eq!(dir, std::path::PathBuf::from("/tmp/test-xdg-state/cozy-desktop"));
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn log_dir_falls_back_to_home() {
        std::env::remove_var("XDG_STATE_HOME");
        let dir = log_dir();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(dir, std::path::PathBuf::from(format!("{home}/.local/state/cozy-desktop")));
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -q logging`
Expected: FAIL — module and function don't exist yet

**Step 3: Implement `log_dir()` and `init()`**

```rust
use std::path::PathBuf;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Returns the XDG-compliant log directory.
///
/// Uses `$XDG_STATE_HOME/cozy-desktop/` if set,
/// otherwise falls back to `$HOME/.local/state/cozy-desktop/`.
pub fn log_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(xdg).join("cozy-desktop")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/state/cozy-desktop")
    } else {
        PathBuf::from("/tmp/cozy-desktop-logs")
    }
}

/// Initialize the dual-output logging system.
///
/// - stderr: human-readable, colored, controlled by `RUST_LOG` (default: `cozy_desktop=info`)
/// - file: JSONL format, daily rotation, always at DEBUG level
///
/// Log files are written to `log_dir()` with the prefix `cozy-desktop`
/// and suffix `.jsonl`, e.g. `cozy-desktop.2026-02-08.jsonl`.
///
/// # Panics
///
/// Panics if the tracing subscriber cannot be initialized (e.g. called twice).
pub fn init() {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok();

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("cozy-desktop")
        .filename_suffix("jsonl")
        .build(&dir)
        .expect("failed to create log file appender");

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .flatten_event(true);

    tracing_subscriber::registry()
        .with(
            stderr_layer.with_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "cozy_desktop=info".into()),
            ),
        )
        .with(
            file_layer.with_filter(
                tracing_subscriber::EnvFilter::new("cozy_desktop=debug"),
            ),
        )
        .init();

    tracing::info!(log_dir = %dir.display(), "⚙️ Logging initialized");
}
```

Note: this needs `use tracing_subscriber::Layer;` for the `.with_filter()` method on individual layers (the `Layer` trait provides it).

**Step 4: Export the module from `src/lib.rs`**

Add `pub mod logging;` to `src/lib.rs`.

**Step 5: Run tests**

Run: `cargo test -q logging`
Expected: PASS

**Step 6: Commit**

```
feat: add logging module with JSONL file output and stderr
```

---

### Task 3: Wire Up `init()` in `main.rs`

**Files:**
- Modify: `src/main.rs`

**Step 1: Replace the existing subscriber setup**

Replace lines 15-24 in `main.rs`:

```rust
// Before:
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
// ...
tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer())
    .with(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "cozy_desktop=info".into()),
    )
    .init();

// After:
cozy_desktop::logging::init();
```

Remove the `tracing_subscriber` import from `main.rs` (no longer needed directly).

**Step 2: Verify it compiles and runs**

Run: `cargo build`
Expected: Compiles

Run: `cargo run -- status 2>/dev/null`
Expected: Runs (may error about config, that's fine)

Verify a `.jsonl` file was created:
```bash
ls ~/.local/state/cozy-desktop/
```

**Step 3: Commit**

```
refactor: use new logging::init() in main
```

---

### Task 4: Add Structured Logging to the Local Scanner

**Files:**
- Modify: `src/local/scanner.rs`

**Step 1: Add tracing instrumentation to `scan()`**

Add structured log events to the scanner:

```rust
// At the start of scan():
tracing::info!(root = %self.root.display(), "🔍 Starting local filesystem scan");

// At the end of scan(), before Ok:
tracing::info!(root = %self.root.display(), count = nodes.len(), "🔍 Scan complete");
```

In `scan_recursive()`:
```rust
// When skipping a symlink/special file:
tracing::debug!(path = %entry_path.display(), "⏭️ Skipping non-regular file");

// When a file changes during hash (TOCTOU):
tracing::debug!(path = %entry_path.display(), "⏭️ File changed during hash, skipping");

// When scanning a directory entry:
tracing::trace!(path = %entry_path.display(), node_type = ?node_type, "🔍 Scanned entry");
```

In `scan_file_with_retries()`:
```rust
// When retrying:
tracing::debug!(path = %path.display(), retries_left, "🔍 File changed during hash, retrying");

// When giving up after retries:
tracing::warn!(path = %path.display(), "⏭️ File unstable after retries, skipping");
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 3: Run tests**

Run: `cargo test -q`
Expected: All tests pass

**Step 4: Commit**

```
feat: add structured logging to local scanner
```

---

### Task 5: Add Structured Logging to the Watcher

**Files:**
- Modify: `src/local/watcher.rs`

**Step 1: Add log events**

In `new()`:
```rust
tracing::info!(root = %root.display(), "👁️ Starting filesystem watcher");
```

In `add_watch_recursive()`:
```rust
tracing::trace!(path = %path.display(), "👁️ Added watch");
```

In `run()`, inside the event loop:
```rust
// For queue overflow:
tracing::warn!("👁️ Inotify queue overflow, full rescan needed");

// For watch invalidation:
tracing::debug!(path = %path.display(), "👁️ Watch invalidated");

// For each dispatched event:
tracing::debug!(path = %path.display(), kind = ?kind, is_dir, "👁️ Filesystem event");

// When receiver is dropped:
tracing::debug!("👁️ Watcher channel closed, stopping");
```

**Step 2: Verify it compiles and tests pass**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to filesystem watcher
```

---

### Task 6: Add Structured Logging to the Planner

**Files:**
- Modify: `src/planner.rs`

**Step 1: Add log events to `plan()`**

```rust
// At the start of plan():
tracing::info!(
    remote_count = remote_nodes.len(),
    local_count = local_nodes.len(),
    synced_count = synced_records.len(),
    "📋 Planning sync operations"
);

// At the end, before Ok:
let op_count = results.iter().filter(|r| matches!(r, PlanResult::Op(_))).count();
let conflict_count = results.iter().filter(|r| matches!(r, PlanResult::Conflict(_))).count();
tracing::info!(
    operations = op_count,
    conflicts = conflict_count,
    "📋 Planning complete"
);
```

In individual planning methods, add debug-level logs for decisions:

```rust
// In plan_all_three, when both changed:
tracing::debug!(
    remote_id = remote.id.as_str(),
    "⚠️ Both sides modified"
);

// In plan_remote_only:
tracing::debug!(
    remote_id = remote.id.as_str(),
    name = &remote.name,
    node_type = ?remote.node_type,
    "📋 New remote node, planning download"
);

// In plan_local_only:
tracing::debug!(
    name = &local.name,
    node_type = ?local.node_type,
    "📋 New local node, planning upload"
);

// In plan_remote_deleted, when conflict:
tracing::debug!(
    remote_id = synced.remote_id.as_str(),
    "⚠️ Remote deleted but local modified"
);

// In plan_local_deleted, when conflict:
tracing::debug!(
    remote_id = remote.id.as_str(),
    "⚠️ Local deleted but remote modified"
);
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to sync planner
```

---

### Task 7: Add Structured Logging to the Sync Engine

**Files:**
- Modify: `src/sync/engine.rs`

**Step 1: Add log events**

In `initial_scan()`:
```rust
tracing::info!(sync_dir = %self.sync_dir.display(), "🔍 Starting initial scan");
// After scan completes:
tracing::info!(count = local_nodes.len(), "🔍 Initial scan found nodes");
```

In `execute_op()`, replace the existing `tracing::warn!` and add per-operation logging:
```rust
// For CreateLocalDir:
tracing::info!(path = %local_path.display(), remote_id = remote_id.as_str(), "📁 Creating local directory");

// For DeleteLocal:
tracing::info!(path = %local_path.display(), "🗑️ Deleting local entry");

// For DeleteRemote:
tracing::info!(remote_id = remote_id.as_str(), "🗑️ Deleting remote entry");

// For unimplemented async ops:
tracing::warn!(op = ?op, "⏭️ Operation requires async execution, skipping");
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to sync engine
```

---

### Task 8: Add Structured Logging to the Remote Client

**Files:**
- Modify: `src/remote/client.rs`

**Step 1: Add log events to API methods**

In `fetch_changes()`:
```rust
tracing::info!(since = ?since, "🌐 Fetching remote changes");
// After parsing response:
tracing::info!(count = results.len(), last_seq = &raw.last_seq, "🌐 Received remote changes");
// For deleted docs:
tracing::debug!(id = &r.id, "🗑️ Remote document deleted");
```

In `download_file()`:
```rust
tracing::info!(file_id = file_id.as_str(), "📥 Downloading file");
tracing::debug!(file_id = file_id.as_str(), size = bytes.len(), "📥 Download complete");
```

In `upload_file()`:
```rust
tracing::info!(parent_id = parent_id.as_str(), name, size = content.len(), "📤 Uploading file");
tracing::debug!(name, "📤 Upload complete");
```

In `create_directory()`:
```rust
tracing::info!(parent_id = parent_id.as_str(), name, "📁 Creating remote directory");
```

In `trash()`:
```rust
tracing::info!(id = id.as_str(), "🗑️ Trashing remote document");
```

In `move_node()`:
```rust
tracing::info!(id = id.as_str(), new_parent_id = new_parent_id.as_str(), new_name, "🔀 Moving remote document");
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to remote client
```

---

### Task 9: Add Structured Logging to Auth

**Files:**
- Modify: `src/remote/auth.rs`

**Step 1: Add log events**

In `register()`:
```rust
tracing::info!(instance_url = normalized_url, client_name, "🔑 Registering OAuth client");
tracing::info!(client_id = &resp.client_id, "🔑 OAuth client registered");
```

In `exchange_code()`:
```rust
tracing::info!("🔑 Exchanging authorization code for tokens");
tracing::info!("🔑 Token exchange successful");
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to OAuth client
```

---

### Task 10: Add Structured Logging to the Store

**Files:**
- Modify: `src/store/tree.rs`

**Step 1: Add log events**

Keep store logs at `trace` level to avoid noise — they're high-frequency:

```rust
// In open():
tracing::info!(path = %path.display(), "💾 Opening tree store");

// In insert_remote_node():
tracing::trace!(id = node.id.as_str(), name = &node.name, "💾 Inserted remote node");

// In delete_remote_node():
tracing::trace!(id = id.as_str(), "💾 Deleted remote node");

// In insert_local_node():
tracing::trace!(name = &node.name, "💾 Inserted local node");

// In delete_local_node():
tracing::trace!("💾 Deleted local node");

// In insert_synced():
tracing::trace!(rel_path = &record.rel_path, "💾 Inserted synced record");

// In delete_synced():
tracing::trace!("💾 Deleted synced record");

// In flush():
tracing::debug!("💾 Store flushed to disk");
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to tree store
```

---

### Task 11: Add Structured Logging to Config

**Files:**
- Modify: `src/config.rs`

**Step 1: Add log events**

```rust
// In load():
tracing::debug!(path = %path.display(), "⚙️ Loading config");

// In save():
tracing::debug!(path = %path.display(), "⚙️ Saving config");
```

**Step 2: Verify**

Run: `cargo test -q`
Expected: All pass

**Step 3: Commit**

```
feat: add structured logging to config
```

---

### Task 12: Enrich `main.rs` Logs with Emojis

**Files:**
- Modify: `src/main.rs`

**Step 1: Add emoji fields to existing `tracing::` calls**

Update all existing `tracing::info!`, `tracing::warn!`, `tracing::error!`, `tracing::debug!` calls in `main.rs` to prefix the message string with the appropriate emoji. For example:

```rust
// cmd_init:
tracing::info!(instance_url, sync_dir = %sync_dir.display(), data_dir = %data_dir.display(), "⚙️ Initialized cozy-desktop");

// cmd_auth:
tracing::info!("🔑 Authentication successful");

// cmd_sync:
tracing::info!("🔄 Starting sync cycle");
tracing::info!(count = ops.len(), "📋 Planned operations");
tracing::info!("🔄 Sync complete");

// cmd_watch:
tracing::info!(sync_dir = %config.sync_dir.display(), "👁️ Watching for changes");
tracing::debug!(event = ?event, "👁️ Watch event received");
tracing::info!("🔄 Changes detected, syncing");
tracing::error!(error = %e, "❌ Sync failed");
tracing::info!("🔄 Periodic sync");
tracing::error!("❌ Watcher disconnected");
```

Also collapse the multi-line init info messages into a single structured event instead of 5 separate `tracing::info!` calls.

**Step 2: Verify**

Run: `cargo build`
Expected: Compiles

**Step 3: Commit**

```
refactor: enrich main.rs logs with emoji fields and structured data
```

---

### Task 13: Final Verification

**Step 1: Run full test suite**

Run: `cargo test -q`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cargo clippy --all-features`
Expected: No new warnings

**Step 3: Run format check**

Run: `cargo fmt --all`

**Step 4: Manual smoke test**

Run: `RUST_LOG=cozy_desktop=debug cargo run -- status 2>&1 | head -20`

Check that stderr output is human-readable (not JSON).

Check that a `.jsonl` file exists in `~/.local/state/cozy-desktop/` and contains valid JSON lines:

```bash
cat ~/.local/state/cozy-desktop/cozy-desktop.*.jsonl | head -5 | python3 -m json.tool
```

**Step 5: Commit**

```
chore: verify structured logging end-to-end
```

---

## Summary of New/Modified Files

| File | Action | What changes |
|------|--------|-------------|
| `Cargo.toml` | Modify | Add `tracing-appender`, `json` feature |
| `src/lib.rs` | Modify | Add `pub mod logging;` |
| `src/logging.rs` | Create | `log_dir()`, `init()` with dual-layer setup |
| `src/main.rs` | Modify | Use `logging::init()`, enrich logs with emojis |
| `src/local/scanner.rs` | Modify | Add scan/skip/TOCTOU log events |
| `src/local/watcher.rs` | Modify | Add watch/event log events |
| `src/planner.rs` | Modify | Add planning decision log events |
| `src/sync/engine.rs` | Modify | Add operation execution log events |
| `src/remote/client.rs` | Modify | Add HTTP request/response log events |
| `src/remote/auth.rs` | Modify | Add OAuth flow log events |
| `src/store/tree.rs` | Modify | Add store operation log events (trace level) |
| `src/config.rs` | Modify | Add config load/save log events |

## JSONL Output Example

Each line in the `.jsonl` file will look like:

```json
{"timestamp":"2026-02-08T14:32:01.234567Z","level":"INFO","target":"cozy_desktop::local::scanner","root":"/home/user/Cozy","count":42,"message":"🔍 Scan complete","filename":"src/local/scanner.rs","line_number":28}
{"timestamp":"2026-02-08T14:32:01.567890Z","level":"INFO","target":"cozy_desktop::planner","operations":3,"conflicts":1,"message":"📋 Planning complete","filename":"src/planner.rs","line_number":30}
{"timestamp":"2026-02-08T14:32:01.890123Z","level":"INFO","target":"cozy_desktop::sync::engine","path":"/home/user/Cozy/docs","remote_id":"abc123","message":"📁 Creating local directory","filename":"src/sync/engine.rs","line_number":80}
```
