# Logging Improvements Design

**Date:** 2026-03-26

## Problem

- The GUI binary never initializes logging — no log files are produced, making GUI-side bugs (e.g. RAG not indexing) impossible to diagnose after the fact.
- The CLI has logging but the content is sparse: reconcile runs say "0 files indexed" with no visibility into why, sync operations are logged as a count but not individually, and the simulator produces no tracing output at all.
- Log lines from different operations cannot be correlated — there are no span fields to group lines from the same sync cycle or reconcile run.

## Goals

1. GUI produces log files identical in format to the CLI.
2. GUI and CLI log files are kept separate (different filename prefix).
3. Key async functions emit structured spans so log lines can be correlated.
4. RAG reconcile logs enough detail to diagnose "nothing was indexed" without a debugger.
5. Simulator emits per-action debug logs so proptest failures produce a readable trace.

## Design

### 1. GUI logging initialization

`logging::init()` in `crates/sync/src/logging.rs` gains a `prefix: &str` parameter (renamed from the hardcoded `"super-ragondin"` string). The signature becomes:

```rust
pub fn init(prefix: &str)
```

- CLI calls `logging::init("super-ragondin-cli")` → writes `super-ragondin-cli.DATE.jsonl`
- GUI calls `logging::init("super-ragondin-gui")` in the Tauri `setup()` closure → writes `super-ragondin-gui.DATE.jsonl`

Both files land in the same XDG state directory (`~/.local/state/super-ragondin/`).

### 2. `#[tracing::instrument]` spans

Add `#[tracing::instrument(...)]` to these functions so every log line emitted inside them carries a parent span field in the JSONL output:

| Function | Crate | Span fields |
|---|---|---|
| `reconcile()` | `super-ragondin-rag` | `synced_count`, `sync_dir` |
| `index_file()` | `super-ragondin-rag` | `rel_path`, `mime_type` |
| `run_cycle_async()` | `super-ragondin-sync` | *(none — grouping only)* |
| `fetch_and_apply_remote_changes()` | `super-ragondin-sync` | `last_seq` |

Use `#[instrument(skip(...))]` to skip non-`Display` arguments (embedder, store, client).

### 3. Targeted log statements

#### RAG indexer (`crates/rag/src/indexer.rs`)

At the top of `reconcile()`, after building the maps:

```
debug!(synced_files, already_indexed, to_delete, to_index, "RAG reconcile plan")
```

Inside `index_file()`:
- After mime detection: `debug!(rel_path, mime_type, "detected mime")`
- After text extraction: `debug!(rel_path, text_len, "extracted text")`
- After chunking: `debug!(rel_path, chunk_count, "chunked text")`
- After embedding: `debug!(rel_path, chunk_count, "embedded chunks")`

In the `reconcile()` file loop, when md5 matches (currently silent):
```
debug!(rel_path, "md5 unchanged, skipping")
```

#### Sync engine (`crates/sync/src/sync/engine.rs`)

After `fetch_and_apply_remote_changes` in both `run_cycle_async()` and the sync loop in CLI/GUI:
```
debug!(changes_applied, last_seq, "applied remote changes")
```

Before executing each planned operation (in the op-dispatch loop):
```
debug!(op = ?op, "executing sync op")
```

#### Simulator (`crates/sync/src/simulator/runner.rs`)

In `apply()`, at the start of each arm:
```
debug!(action = %action, "sim apply")
```

`SimAction` already has `Display` via its enum structure; if not, derive/implement it.

In `check_all_invariants()`, when any check fails:
```
warn!(invariant, error = %e, "invariant violated")
```

## File changes

| File | Change |
|---|---|
| `crates/sync/src/logging.rs` | Add `prefix: &str` parameter to `init()` |
| `crates/cli/src/main.rs` | Pass `"super-ragondin-cli"` to `logging::init()` |
| `crates/gui/src/main.rs` | Call `logging::init("super-ragondin-gui")` in setup |
| `crates/rag/src/indexer.rs` | Add `#[instrument]` to `reconcile` and `index_file`; add targeted debug events |
| `crates/sync/src/sync/engine.rs` | Add `#[instrument]` to `run_cycle_async` and `fetch_and_apply_remote_changes`; add per-op debug log |
| `crates/sync/src/simulator/runner.rs` | Add per-action debug log in `apply()`; add warn log in `check_all_invariants()` |

## Testing

- Existing unit tests for `log_dir()` continue to pass unchanged.
- No new tests required: the changes are purely additive log statements and a signature change with trivial callers.
