# Parallel Transfers Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Run up to 2 file downloads/uploads concurrently during a sync cycle, instead of one at a time.

**Architecture:** The planner already sorts operations as: create-dirs → moves → transfers → deletes. All transfer methods (`execute_download_new`, `execute_download_update`, `execute_upload_new`, `execute_upload_update`) take `&self`, not `&mut self`, because `TreeStore` (fjall) is internally thread-safe. We relax `execute_op`/`execute_op_async` to `&self`, then use `futures::stream::buffer_unordered(2)` to run consecutive transfer operations concurrently while keeping other operations sequential.

**Tech Stack:** `futures` crate (for `StreamExt::buffer_unordered`)

---

### Task 1: Add `SyncOp::is_transfer()` method

**Files:**
- Modify: `src/model.rs` (add method on `SyncOp`)

**Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `src/model.rs`:

```rust
#[test]
fn sync_op_is_transfer() {
    let download = SyncOp::DownloadNew {
        remote_id: RemoteId::new("r1"),
        local_path: PathBuf::from("/tmp/f"),
        expected_rev: "1-x".to_string(),
        expected_md5: "abc".to_string(),
    };
    assert!(download.is_transfer());

    let upload = SyncOp::UploadNew {
        local_id: LocalFileId::new(1, 1),
        local_path: PathBuf::from("/tmp/f"),
        parent_remote_id: RemoteId::new("p"),
        name: "f".to_string(),
        expected_md5: "abc".to_string(),
    };
    assert!(upload.is_transfer());

    let create_dir = SyncOp::CreateLocalDir {
        remote_id: RemoteId::new("d1"),
        local_path: PathBuf::from("/tmp/d"),
    };
    assert!(!create_dir.is_transfer());

    let delete = SyncOp::DeleteRemote {
        remote_id: RemoteId::new("r1"),
        expected_rev: "1-x".to_string(),
    };
    assert!(!delete.is_transfer());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -q sync_op_is_transfer`
Expected: FAIL — `is_transfer` method doesn't exist yet.

**Step 3: Write minimal implementation**

Add this `impl` block on `SyncOp` in `src/model.rs` (after the enum definition, before the `Conflict` struct):

```rust
impl SyncOp {
    /// Returns true for file transfer operations (downloads and uploads).
    #[must_use]
    pub const fn is_transfer(&self) -> bool {
        matches!(
            self,
            Self::DownloadNew { .. }
                | Self::DownloadUpdate { .. }
                | Self::UploadNew { .. }
                | Self::UploadUpdate { .. }
        )
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -q sync_op_is_transfer`
Expected: PASS

**Step 5: Run formatters and linters**

Run: `cargo fmt --all && cargo clippy --all-features`
Expected: No warnings.

**Step 6: Commit**

```bash
git add src/model.rs
git commit -m "feat: add SyncOp::is_transfer() method"
```

---

### Task 2: Relax `execute_op` and `execute_op_async` to `&self`

Both methods currently take `&mut self`, but every inner method they call takes `&self` (fjall store is thread-safe). Relaxing the signature enables concurrent calls.

**Files:**
- Modify: `src/sync/engine.rs`

**Step 1: Change `execute_op` from `&mut self` to `&self`**

In `src/sync/engine.rs`, change the signature of `execute_op` (around line 248):

```rust
// Before:
pub fn execute_op(&mut self, op: &SyncOp) -> Result<()> {
// After:
pub fn execute_op(&self, op: &SyncOp) -> Result<()> {
```

**Step 2: Change `execute_op_async` from `&mut self` to `&self`**

In `src/sync/engine.rs`, change the signature of `execute_op_async` (around line 145):

```rust
// Before:
pub async fn execute_op_async(&mut self, client: &CozyClient, op: &SyncOp) -> Result<()> {
// After:
pub async fn execute_op_async(&self, client: &CozyClient, op: &SyncOp) -> Result<()> {
```

**Step 3: Run all tests to verify nothing breaks**

Run: `cargo test -q`
Expected: All tests PASS — callers already have `&mut self` which auto-dereferences to `&self`.

**Step 4: Run formatters and linters**

Run: `cargo fmt --all && cargo clippy --all-features`
Expected: No warnings.

**Step 5: Commit**

```bash
git add src/sync/engine.rs
git commit -m "refactor: relax execute_op and execute_op_async to &self"
```

---

### Task 3: Add `futures` dependency

**Step 1: Add the dependency**

Run: `cargo add futures`

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add futures crate for stream concurrency"
```

---

### Task 4: Refactor `run_cycle_async` for parallel transfers

**Files:**
- Modify: `src/sync/engine.rs`

**Step 1: Write a failing test for multiple concurrent downloads**

Add to `tests/sync_tests.rs`:

```rust
#[tokio::test]
async fn test_sync_engine_parallel_downloads() {
    use cozy_desktop::model::{PlanResult, SyncOp};
    use cozy_desktop::remote::client::CozyClient;
    use std::os::unix::fs::MetadataExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock two download endpoints
    let content_a = b"content of file A";
    let content_b = b"content of file B";

    // md5 of "content of file A"
    let md5_a = format!("{:x}", md5::Md5::digest(content_a));
    // md5 of "content of file B"
    let md5_b = format!("{:x}", md5::Md5::digest(content_b));

    Mock::given(method("GET"))
        .and(path("/files/download/file-a"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content_a.as_slice()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/files/download/file-b"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content_b.as_slice()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Set up root
    let sync_meta = std::fs::metadata(sync_dir.path()).unwrap();
    let root_local_id = LocalFileId::new(sync_meta.dev(), sync_meta.ino());
    let root_synced = SyncedRecord {
        local_id: root_local_id.clone(),
        remote_id: RemoteId::new("io.cozy.files.root-dir"),
        rel_path: String::new(),
        md5sum: None,
        size: None,
        rev: "1-root".to_string(),
        node_type: NodeType::Directory,
        local_name: Some(String::new()),
        local_parent_id: None,
        remote_name: Some(String::new()),
        remote_parent_id: None,
    };
    store.insert_synced(&root_synced).unwrap();

    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root).unwrap();

    let root_local = cozy_desktop::model::LocalNode {
        id: root_local_id,
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 0,
    };
    store.insert_local_node(&root_local).unwrap();

    // Add two remote files
    let file_a = RemoteNode {
        id: RemoteId::new("file-a"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "a.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5_a.clone()),
        size: Some(content_a.len() as u64),
        updated_at: 1000,
        rev: "1-a".to_string(),
    };
    let file_b = RemoteNode {
        id: RemoteId::new("file-b"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "b.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5_b.clone()),
        size: Some(content_b.len() as u64),
        updated_at: 1000,
        rev: "1-b".to_string(),
    };
    store.insert_remote_node(&file_a).unwrap();
    store.insert_remote_node(&file_b).unwrap();
    store.flush().unwrap();

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let results = engine.run_cycle_async(&client).await.unwrap();

    // Both downloads should have been planned
    let download_count = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })))
        .count();
    assert_eq!(download_count, 2, "Should have planned 2 downloads");

    // Both files should exist on disk
    assert_eq!(
        std::fs::read_to_string(sync_dir.path().join("a.txt")).unwrap(),
        "content of file A"
    );
    assert_eq!(
        std::fs::read_to_string(sync_dir.path().join("b.txt")).unwrap(),
        "content of file B"
    );

    // Both synced records should exist
    assert!(engine
        .store()
        .get_synced_by_remote(&RemoteId::new("file-a"))
        .unwrap()
        .is_some());
    assert!(engine
        .store()
        .get_synced_by_remote(&RemoteId::new("file-b"))
        .unwrap()
        .is_some());
}
```

Note: you will need to add `use md5::Digest;` at the top of the test (the `md5` crate is already available as `md-5` in Cargo.toml, imported as `md5`).

**Step 2: Run the test to verify it passes with current sequential code**

Run: `cargo test -q test_sync_engine_parallel_downloads`
Expected: PASS (this test validates correctness, not concurrency — it passes with sequential execution too).

**Step 3: Refactor `run_cycle_async` for concurrent transfers**

Replace the `run_cycle_async` method in `src/sync/engine.rs` with:

```rust
pub async fn run_cycle_async(&mut self, client: &CozyClient) -> Result<Vec<PlanResult>> {
    use futures::stream::{self, StreamExt as _};

    tracing::info!("🔄 Starting async sync cycle");
    self.initial_scan()?;

    let results = self.plan()?;
    let op_count = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(_)))
        .count();
    tracing::info!(operations = op_count, "📋 Planned operations");

    let ops: Vec<&SyncOp> = results
        .iter()
        .filter_map(|r| match r {
            PlanResult::Op(op) => Some(op),
            PlanResult::Conflict(conflict) => {
                tracing::warn!(conflict = ?conflict, "⚠️ Conflict");
                None
            }
            PlanResult::NoOp => None,
        })
        .collect();

    let mut i = 0;
    while i < ops.len() {
        if ops[i].is_transfer() {
            let start = i;
            while i < ops.len() && ops[i].is_transfer() {
                i += 1;
            }
            let engine_ref = &*self;
            let mut stream = stream::iter(&ops[start..i])
                .map(|op| engine_ref.execute_op_async(client, op))
                .buffer_unordered(2);
            while let Some(result) = stream.next().await {
                result?;
            }
        } else {
            self.execute_op_async(client, ops[i]).await?;
            i += 1;
        }
    }

    tracing::info!("🔄 Async sync cycle complete");
    Ok(results)
}
```

**Step 4: Run all tests**

Run: `cargo test -q`
Expected: All tests PASS.

**Step 5: Run formatters and linters**

Run: `cargo fmt --all && cargo clippy --all-features`
Expected: No warnings.

**Step 6: Commit**

```bash
git add src/sync/engine.rs tests/sync_tests.rs
git commit -m "feat: run up to 2 file transfers concurrently"
```

---

## Summary of changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `futures` dependency |
| `src/model.rs` | Add `SyncOp::is_transfer()` method + test |
| `src/sync/engine.rs` | Relax `execute_op`/`execute_op_async` to `&self`; refactor `run_cycle_async` to use `buffer_unordered(2)` for transfer ops |
| `tests/sync_tests.rs` | Add `test_sync_engine_parallel_downloads` |

## Design notes

- **Why `buffer_unordered(2)`?** It runs at most 2 futures concurrently, completing them in any order. This limits network concurrency without needing a semaphore.
- **Why group consecutive transfers?** The planner sorts ops as: create-dirs(0) → moves(1) → transfers(2) → deletes(3). Grouping preserves this ordering — directories are created before files are downloaded into them, and deletes happen after transfers.
- **Error handling:** If any transfer fails, `result?` breaks out of the stream loop, dropping remaining in-flight futures. This matches the existing sequential behavior.
- **Thread safety:** `TreeStore` uses fjall which is internally thread-safe. All transfer methods take `&self`. The explicit reborrow (`let engine_ref = &*self`) allows multiple concurrent `&self` borrows from within the `&mut self` method.
