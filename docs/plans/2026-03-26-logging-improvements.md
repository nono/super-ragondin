# Logging Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add log files to the GUI binary and enrich log content in the RAG indexer, sync engine, and simulator so debugging is practical without a debugger.

**Architecture:** Three independent areas of change: (1) the `logging::init()` function gains a `prefix` parameter so CLI and GUI write separate log files; (2) `#[tracing::instrument]` attributes on key functions add span correlation to the JSONL output; (3) targeted `tracing::debug!` statements fill in per-item detail at extraction, chunking, embedding, and per-op dispatch points.

**Tech Stack:** `tracing` 0.1 (`#[instrument]` macro included by default), `tracing-appender` 0.2, Tauri v2

---

### Task 1: Add `prefix` parameter to `logging::init()`

**Files:**
- Modify: `crates/sync/src/logging.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Confirm the existing test passes before touching anything**

```bash
cargo test -q -p super-ragondin-sync logging
```

Expected output: `test logging::tests::log_dir_ends_with_super_ragondin ... ok`

- [ ] **Step 2: Update `init()` signature and body**

In `crates/sync/src/logging.rs`, replace the entire `init` function:

```rust
/// Initialize the dual-output logging system.
///
/// - stderr: human-readable, colored, controlled by `RUST_LOG` (default: `super_ragondin*=info`)
/// - file: JSONL format, daily rotation, always at DEBUG level
///
/// Log files are written to `log_dir()` with the given `prefix`
/// and suffix `.jsonl`, e.g. `super-ragondin-cli.2026-02-08.jsonl`.
///
/// # Panics
///
/// Panics if the tracing subscriber cannot be initialized (e.g. called twice).
pub fn init(prefix: &str) {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok();

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("jsonl")
        .build(&dir)
        .expect("failed to create log file appender");

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

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
                    .unwrap_or_else(|_| STDERR_LOG_FILTER.into()),
            ),
        )
        .with(file_layer.with_filter(tracing_subscriber::EnvFilter::new(FILE_LOG_FILTER)))
        .init();

    tracing::info!(log_dir = %dir.display(), prefix, "⚙️ Logging initialized");
}
```

- [ ] **Step 3: Update the CLI caller**

In `crates/cli/src/main.rs`, change line 19:

```rust
super_ragondin_sync::logging::init("super-ragondin-cli");
```

- [ ] **Step 4: Verify it compiles and the test still passes**

```bash
cargo test -q -p super-ragondin-sync logging && cargo build -p super-ragondin -q
```

Expected: test passes, binary builds with no errors.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/sync/src/logging.rs crates/cli/src/main.rs
git commit -m "refactor(logging): accept prefix param so CLI and GUI write separate log files"
```

---

### Task 2: Initialize logging in the GUI

**Files:**
- Modify: `crates/gui/src/main.rs`

- [ ] **Step 1: Add `logging::init()` call in the Tauri `setup()` closure**

In `crates/gui/src/main.rs`, the `setup` closure starts at line 22. Add the `logging::init` call as the very first line of the setup closure, before the `#[cfg(feature = "tray")]` block:

```rust
        .setup(move |app| {
            super_ragondin_sync::logging::init("super-ragondin-gui");
            builder.mount_events(app);
            // ... rest unchanged
```

- [ ] **Step 2: Verify the GUI crate compiles**

```bash
cargo build -p super-ragondin-gui --no-default-features --features custom-protocol -q
```

Expected: builds with no errors.

- [ ] **Step 3: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/gui/src/main.rs
git commit -m "feat(gui): initialize logging so GUI writes log files to ~/.local/state/super-ragondin/"
```

---

### Task 3: Add `#[instrument]` spans and targeted logs to the RAG indexer

**Files:**
- Modify: `crates/rag/src/indexer.rs`

- [ ] **Step 1: Verify existing RAG tests pass**

```bash
cargo test -q -p super-ragondin-rag
```

Expected: all tests pass.

- [ ] **Step 2: Add `#[instrument]` to `reconcile()`**

Replace the `reconcile` function signature (line 17) with:

```rust
#[tracing::instrument(skip(synced, rag_store, embedder), fields(synced_count = synced.len(), sync_dir = %sync_dir.display()))]
pub async fn reconcile(
    synced: &[SyncedRecord],
    sync_dir: &Path,
    rag_store: &RagStore,
    embedder: &dyn Embedder,
) -> Result<()> {
```

- [ ] **Step 3: Add reconcile plan summary log**

After the two maps are built (after line 31, before the delete loop), add:

```rust
    let delete_count = indexed_map
        .keys()
        .filter(|k| !synced_map.contains_key(k.as_str()))
        .count();
    let reindex_count = synced_map
        .iter()
        .filter(|(rel_path, md5sum)| {
            indexed_map.get(*rel_path).map(String::as_str) != Some(md5sum)
        })
        .count();
    tracing::debug!(
        synced_files = synced_map.len(),
        already_indexed = indexed_map.len(),
        to_delete = delete_count,
        to_index = reindex_count,
        "RAG reconcile plan"
    );
```

- [ ] **Step 4: Add md5-unchanged skip log**

Inside the `for (rel_path, md5sum) in &synced_map` loop, replace the silent `continue` (line 42):

```rust
        if indexed_map.get(*rel_path).map(String::as_str) == Some(md5sum) {
            tracing::debug!(rel_path, "md5 unchanged, skipping");
            continue;
        }
```

- [ ] **Step 5: Add `#[instrument]` to `index_file()`**

Replace the `index_file` function signature (line 120) with:

```rust
#[tracing::instrument(skip(file_path, mtime, md5sum, embedder), fields(rel_path, mime_type))]
async fn index_file(
    rel_path: &str,
    file_path: &Path,
    mime_type: &str,
    mtime: i64,
    md5sum: &str,
    embedder: &dyn Embedder,
) -> Result<Vec<ChunkRecord>> {
```

- [ ] **Step 6: Add per-stage detail logs inside `index_file()`**

After the text extraction match, before chunking, add a log for extracted text length. Find the line `Some(text) => chunker::chunk_text(&text, mime_type)?,` (inside the non-image branch) and replace it:

```rust
                Some(text) => {
                    tracing::debug!(text_len = text.len(), "extracted text");
                    let chunks = chunker::chunk_text(&text, mime_type)?;
                    tracing::debug!(chunk_count = chunks.len(), "chunked text");
                    chunks
                }
```

Before the `embedder.embed_texts` call (line 164), capture the count and log after embedding:

```rust
    let chunk_count = texts.len();
    let embeddings = embedder.embed_texts(&texts).await?;
    tracing::debug!(chunk_count, "embedded chunks");
```

- [ ] **Step 7: Run RAG tests to confirm nothing broke**

```bash
cargo test -q -p super-ragondin-rag
```

Expected: all tests pass.

- [ ] **Step 8: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/rag/src/indexer.rs
git commit -m "feat(rag): add instrument spans and detailed debug logs to reconcile and index_file"
```

---

### Task 4: Add `#[instrument]` and per-op log to the sync engine

**Files:**
- Modify: `crates/sync/src/sync/engine.rs`

- [ ] **Step 1: Verify sync tests pass**

```bash
cargo test -q -p super-ragondin-sync
```

Expected: all tests pass.

- [ ] **Step 2: Add `#[instrument]` to `fetch_and_apply_remote_changes()`**

Add the attribute above the function at line 247:

```rust
    #[tracing::instrument(skip(self, client), fields(since))]
    pub async fn fetch_and_apply_remote_changes(
        &self,
        client: &CozyClient,
        since: Option<&str>,
    ) -> Result<String> {
```

Then add a count log just before `Ok(changes.last_seq)` (after the `self.store.flush()?` line):

```rust
        self.store.flush()?;
        tracing::debug!(changes_applied = changes.results.len(), last_seq = %changes.last_seq, "applied remote changes");
        Ok(changes.last_seq)
```

- [ ] **Step 3: Add `#[instrument]` to `run_cycle_async()`**

Add the attribute above the function at line 344:

```rust
    #[tracing::instrument(skip(self, client))]
    pub async fn run_cycle_async(&mut self, client: &CozyClient) -> Result<Vec<PlanResult>> {
```

- [ ] **Step 4: Add per-op debug log in `execute_op_async()`**

At the top of `execute_op_async` (line 442), before the match, add:

```rust
    pub async fn execute_op_async(&self, client: &CozyClient, op: &SyncOp) -> Result<()> {
        tracing::debug!(op = ?op, "executing sync op");
        match op {
```

- [ ] **Step 5: Add per-op debug log in `execute_op()` too**

Find `pub fn execute_op(&mut self, op: &SyncOp) -> Result<()>` and add the same log at the top of its match:

```rust
    pub fn execute_op(&mut self, op: &SyncOp) -> Result<()> {
        tracing::debug!(op = ?op, "executing sync op");
        match op {
```

- [ ] **Step 6: Run sync tests**

```bash
cargo test -q -p super-ragondin-sync
```

Expected: all tests pass.

- [ ] **Step 7: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/sync/src/sync/engine.rs
git commit -m "feat(sync): add instrument spans to run_cycle_async and fetch_and_apply; log per-op at debug level"
```

---

### Task 5: Add per-action debug logs to the simulator

**Files:**
- Modify: `crates/sync/src/simulator/runner.rs`

- [ ] **Step 1: Verify simulator/sync tests pass**

```bash
cargo test -q -p super-ragondin-sync
```

Expected: all tests pass.

- [ ] **Step 2: Add per-action debug log at the top of `apply()`**

`SimAction` already implements `Display`. Add one log line at the very start of the `apply` method body (line 290), before the `match`:

```rust
    pub fn apply(&mut self, action: SimAction) -> Result<(), String> {
        tracing::debug!(action = %action, "sim apply");
        match action {
```

- [ ] **Step 3: Add warn log in `check_all_invariants()`**

Find `check_all_invariants` (line 2234). It calls each checker and collects errors. Add a warn log when an invariant fails. Replace the body pattern with something that logs each failure. The current body likely looks like:

```rust
    pub fn check_all_invariants(&self) -> Result<(), String> {
        let checks: &[fn(&Self) -> Result<(), String>] = &[
            Self::check_convergence,
            Self::check_store_consistency,
            Self::check_idempotency,
            Self::check_content_integrity,
            Self::check_no_orphaned_store_nodes,
            Self::check_no_duplicate_local_paths,
        ];
        let mut errors = Vec::new();
        for check in checks {
            if let Err(e) = check(self) {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n\n"))
        }
    }
```

Read the actual body first (it may differ), then add a `tracing::warn!` inside the `Err(e)` arm:

```rust
        for check in checks {
            if let Err(e) = check(self) {
                tracing::warn!(error = %e, "invariant violated");
                errors.push(e);
            }
        }
```

- [ ] **Step 4: Read the actual `check_all_invariants` body before editing**

Read lines 2234–2260 of `crates/sync/src/simulator/runner.rs` to see the real structure, then apply the `tracing::warn!` insertion accurately.

- [ ] **Step 5: Run sync tests**

```bash
cargo test -q -p super-ragondin-sync
```

Expected: all tests pass.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/sync/src/simulator/runner.rs
git commit -m "feat(simulator): log each sim action at debug level and warn on invariant violations"
```
