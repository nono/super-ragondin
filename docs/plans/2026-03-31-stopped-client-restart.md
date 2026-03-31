# Stopped-Client Restart Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `SyncEngine::initial_scan` to remove stale local nodes so that files deleted or replaced while the client was stopped are correctly detected and synced on the next restart.

**Architecture:** `initial_scan` in `crates/sync/src/sync/engine.rs` is extended with a single stale-node removal pass after inserting scanned nodes. The existing safety check (abort on empty scan with known synced files) remains in place and runs before any deletion. Six new tests are added: three unit tests against the real engine+filesystem in `sync_tests.rs`, and three simulation tests in `simulator_tests.rs`.

**Tech Stack:** Rust, fjall (TreeStore), tempfile (tests), existing `SimulationRunner` test helpers.

---

## Files

| Action | Path | What changes |
|---|---|---|
| Modify | `crates/sync/src/sync/engine.rs` | `initial_scan`: add stale-node removal after inserting scanned nodes |
| Modify | `crates/sync/tests/sync_tests.rs` | Add 3 unit tests |
| Modify | `crates/sync/tests/simulator_tests.rs` | Add 3 simulation tests |

---

### Task 1: Unit test — delete while stopped (red)

**Files:**
- Modify: `crates/sync/tests/sync_tests.rs`

- [ ] **Step 1: Add the failing test**

Append to `crates/sync/tests/sync_tests.rs`:

```rust
#[test]
fn initial_scan_detects_file_deleted_while_stopped() {
    use std::os::unix::fs::MetadataExt;
    use super_ragondin_sync::model::{LocalNode, PlanResult, SyncOp};
    use super_ragondin_sync::util::compute_md5_from_bytes;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    // Write the file before "stopping" so we can capture its real inode.
    let file_path = sync_dir.path().join("synced.txt");
    let content = b"hello";
    std::fs::write(&file_path, content).unwrap();

    let root_meta = std::fs::symlink_metadata(sync_dir.path()).unwrap();
    let root_local_id = LocalFileId::new(root_meta.dev(), root_meta.ino());
    let file_meta = std::fs::symlink_metadata(&file_path).unwrap();
    let file_local_id = LocalFileId::new(file_meta.dev(), file_meta.ino());
    let md5 = compute_md5_from_bytes(content);

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Root
    store.insert_local_node(&LocalNode {
        id: root_local_id.clone(),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: root_meta.mtime(),
    }).unwrap();
    store.insert_remote_node(&RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    }).unwrap();
    store.insert_synced(&SyncedRecord {
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
    }).unwrap();

    // Previously-synced file
    store.insert_local_node(&LocalNode {
        id: file_local_id.clone(),
        parent_id: Some(root_local_id.clone()),
        name: "synced.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.clone()),
        size: Some(content.len() as u64),
        mtime: file_meta.mtime(),
    }).unwrap();
    store.insert_remote_node(&RemoteNode {
        id: RemoteId::new("remote-synced-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "synced.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.clone()),
        size: Some(content.len() as u64),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    }).unwrap();
    store.insert_synced(&SyncedRecord {
        local_id: file_local_id.clone(),
        remote_id: RemoteId::new("remote-synced-1"),
        rel_path: "synced.txt".to_string(),
        md5sum: Some(md5),
        size: Some(content.len() as u64),
        rev: "1-abc".to_string(),
        node_type: NodeType::File,
        local_name: Some("synced.txt".to_string()),
        local_parent_id: Some(root_local_id.clone()),
        remote_name: Some("synced.txt".to_string()),
        remote_parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
    }).unwrap();
    store.flush().unwrap();

    // Delete the file while "stopped"
    std::fs::remove_file(&file_path).unwrap();

    // Restart: fresh engine, run initial_scan
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
        IgnoreRules::none(),
    );
    engine.initial_scan().unwrap();

    // Stale local node must be gone
    assert!(
        engine.store().get_local_node(&file_local_id).unwrap().is_none(),
        "initial_scan should remove stale local node for file deleted while stopped"
    );

    // Plan must contain DeleteRemote (not NoOp)
    let results = engine.plan().unwrap();
    let has_delete = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::DeleteRemote { remote_id, .. })
            if remote_id.as_str() == "remote-synced-1")
    });
    assert!(has_delete, "plan should contain DeleteRemote for file deleted while stopped");
}
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo test -q initial_scan_detects_file_deleted_while_stopped
```

Expected: FAIL — assertion "initial_scan should remove stale local node" fires because the stale node is still in the store.

---

### Task 2: Unit test — replace while stopped (red)

**Files:**
- Modify: `crates/sync/tests/sync_tests.rs`

- [ ] **Step 1: Add the failing test**

Append to `crates/sync/tests/sync_tests.rs`:

```rust
#[test]
fn initial_scan_detects_file_replaced_while_stopped() {
    use std::os::unix::fs::MetadataExt;
    use super_ragondin_sync::model::{Conflict, LocalNode, PlanResult, SyncOp};
    use super_ragondin_sync::util::compute_md5_from_bytes;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    // Write original file to get its inode.
    let file_path = sync_dir.path().join("replaced.txt");
    let old_content = b"original";
    std::fs::write(&file_path, old_content).unwrap();

    let root_meta = std::fs::symlink_metadata(sync_dir.path()).unwrap();
    let root_local_id = LocalFileId::new(root_meta.dev(), root_meta.ino());
    let old_file_meta = std::fs::symlink_metadata(&file_path).unwrap();
    let old_file_local_id = LocalFileId::new(old_file_meta.dev(), old_file_meta.ino());
    let old_md5 = compute_md5_from_bytes(old_content);

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Root
    store.insert_local_node(&LocalNode {
        id: root_local_id.clone(),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: root_meta.mtime(),
    }).unwrap();
    store.insert_remote_node(&RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    }).unwrap();
    store.insert_synced(&SyncedRecord {
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
    }).unwrap();

    // Previously-synced file (old version)
    store.insert_local_node(&LocalNode {
        id: old_file_local_id.clone(),
        parent_id: Some(root_local_id.clone()),
        name: "replaced.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(old_md5.clone()),
        size: Some(old_content.len() as u64),
        mtime: old_file_meta.mtime(),
    }).unwrap();
    store.insert_remote_node(&RemoteNode {
        id: RemoteId::new("remote-replaced-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "replaced.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(old_md5.clone()),
        size: Some(old_content.len() as u64),
        updated_at: 1000,
        rev: "1-orig".to_string(),
    }).unwrap();
    store.insert_synced(&SyncedRecord {
        local_id: old_file_local_id.clone(),
        remote_id: RemoteId::new("remote-replaced-1"),
        rel_path: "replaced.txt".to_string(),
        md5sum: Some(old_md5),
        size: Some(old_content.len() as u64),
        rev: "1-orig".to_string(),
        node_type: NodeType::File,
        local_name: Some("replaced.txt".to_string()),
        local_parent_id: Some(root_local_id.clone()),
        remote_name: Some("replaced.txt".to_string()),
        remote_parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
    }).unwrap();
    store.flush().unwrap();

    // Replace the file while "stopped": delete + create gives a new inode
    std::fs::remove_file(&file_path).unwrap();
    std::fs::write(&file_path, b"replacement").unwrap();

    // Restart: fresh engine, run initial_scan
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
        IgnoreRules::none(),
    );
    engine.initial_scan().unwrap();

    // Stale local node for old file must be gone
    assert!(
        engine.store().get_local_node(&old_file_local_id).unwrap().is_none(),
        "initial_scan should remove stale local node for replaced file"
    );

    let results = engine.plan().unwrap();

    // Must NOT produce a NameCollision conflict for the new file
    let has_name_collision = results.iter().any(|r| {
        matches!(r, PlanResult::Conflict(Conflict { kind: ConflictKind::NameCollision, .. }))
    });
    assert!(!has_name_collision, "replaced file should not produce NameCollision conflict");

    // Must plan DeleteRemote for the old remote file
    let has_delete = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::DeleteRemote { remote_id, .. })
            if remote_id.as_str() == "remote-replaced-1")
    });
    assert!(has_delete, "plan should contain DeleteRemote for old file");

    // Must plan UploadNew for the new file
    let has_upload = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::UploadNew { name, .. }) if name == "replaced.txt")
    });
    assert!(has_upload, "plan should contain UploadNew for the replacement file");
}
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo test -q initial_scan_detects_file_replaced_while_stopped
```

Expected: FAIL — stale node still in store causes NameCollision conflict.

---

### Task 3: Fix `initial_scan` (green)

**Files:**
- Modify: `crates/sync/src/sync/engine.rs:1` (imports)
- Modify: `crates/sync/src/sync/engine.rs:68` (`initial_scan` body)

- [ ] **Step 1: Add `HashSet` import**

In `crates/sync/src/sync/engine.rs`, add to the existing `use std::` imports:

```rust
use std::collections::HashSet;
```

- [ ] **Step 2: Replace the body of `initial_scan`**

Find and replace the entire `initial_scan` method (lines 68–105):

Old:
```rust
    pub fn initial_scan(&mut self) -> Result<()> {
        tracing::info!(sync_dir = %self.sync_dir.display(), "🔍 Starting initial scan");

        self.bootstrap_root()?;

        let scanner = Scanner::new(&self.sync_dir);
        let local_nodes = scanner.scan_with_ignore(&self.rules)?;
        let count = local_nodes.len();

        // Safety check: if the scanner found zero files but we have previously
        // synced records, the sync directory is likely on an unmounted drive or
        // otherwise inaccessible.  Abort to prevent the planner from generating
        // DeleteRemote ops for every known file, which would wipe the user's
        // remote data.
        let non_root_synced = self
            .store
            .list_all_synced()?
            .into_iter()
            .filter(|r| !r.rel_path.is_empty())
            .count();
        if local_nodes.is_empty() && non_root_synced > 0 {
            tracing::error!(
                synced_count = non_root_synced,
                "🚨 Sync directory appears empty but {non_root_synced} files are known — aborting"
            );
            return Err(Error::EmptySyncDir {
                synced_count: non_root_synced,
            });
        }

        for node in &local_nodes {
            self.store.insert_local_node(node)?;
        }

        tracing::info!(count, "🔍 Initial scan found nodes");
        self.store.flush()?;
        Ok(())
    }
```

New:
```rust
    pub fn initial_scan(&mut self) -> Result<()> {
        tracing::info!(sync_dir = %self.sync_dir.display(), "🔍 Starting initial scan");

        self.bootstrap_root()?;

        let scanner = Scanner::new(&self.sync_dir);
        let local_nodes = scanner.scan_with_ignore(&self.rules)?;
        let count = local_nodes.len();

        // Safety check: if the scanner found zero files but we have previously
        // synced records, the sync directory is likely on an unmounted drive or
        // otherwise inaccessible.  Abort to prevent the planner from generating
        // DeleteRemote ops for every known file, which would wipe the user's
        // remote data.
        let non_root_synced = self
            .store
            .list_all_synced()?
            .into_iter()
            .filter(|r| !r.rel_path.is_empty())
            .count();
        if local_nodes.is_empty() && non_root_synced > 0 {
            tracing::error!(
                synced_count = non_root_synced,
                "🚨 Sync directory appears empty but {non_root_synced} files are known — aborting"
            );
            return Err(Error::EmptySyncDir {
                synced_count: non_root_synced,
            });
        }

        for node in &local_nodes {
            self.store.insert_local_node(node)?;
        }

        // Remove stale local nodes — nodes that were in the store from a previous
        // session but are no longer present on disk (e.g. deleted while stopped).
        // The root node is not returned by the scanner so we preserve it explicitly.
        let root_meta = fs::symlink_metadata(&self.sync_dir)?;
        let root_local_id = LocalFileId::new(root_meta.dev(), root_meta.ino());
        let scanned_ids: HashSet<LocalFileId> = local_nodes
            .iter()
            .map(|n| n.id.clone())
            .chain(std::iter::once(root_local_id))
            .collect();
        let stale_ids: Vec<LocalFileId> = self
            .store
            .list_all_local()?
            .into_iter()
            .map(|n| n.id)
            .filter(|id| !scanned_ids.contains(id))
            .collect();
        if !stale_ids.is_empty() {
            tracing::info!(count = stale_ids.len(), "🧹 Removing stale local nodes");
            for id in &stale_ids {
                self.store.delete_local_node(id)?;
            }
        }

        tracing::info!(count, "🔍 Initial scan found nodes");
        self.store.flush()?;
        Ok(())
    }
```

- [ ] **Step 3: Run the two failing tests, confirm they now pass**

```bash
cargo test -q initial_scan_detects_file_deleted_while_stopped initial_scan_detects_file_replaced_while_stopped
```

Expected: both PASS.

- [ ] **Step 4: Run fmt and clippy**

```bash
cargo fmt --all
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/sync/src/sync/engine.rs
git commit -m "fix: remove stale local nodes in initial_scan after restart"
```

---

### Task 4: Unit test — add while stopped (documents existing behaviour)

**Files:**
- Modify: `crates/sync/tests/sync_tests.rs`

- [ ] **Step 1: Add the test**

Append to `crates/sync/tests/sync_tests.rs`:

```rust
#[test]
fn initial_scan_picks_up_file_added_while_stopped() {
    use std::os::unix::fs::MetadataExt;
    use super_ragondin_sync::model::{LocalNode, PlanResult, SyncOp};

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let root_meta = std::fs::symlink_metadata(sync_dir.path()).unwrap();
    let root_local_id = LocalFileId::new(root_meta.dev(), root_meta.ino());

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Root
    store.insert_local_node(&LocalNode {
        id: root_local_id.clone(),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: root_meta.mtime(),
    }).unwrap();
    store.insert_remote_node(&RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    }).unwrap();
    store.insert_synced(&SyncedRecord {
        local_id: root_local_id,
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
    }).unwrap();
    store.flush().unwrap();

    // Add a file while the client is "stopped"
    std::fs::write(sync_dir.path().join("offline.txt"), b"created while stopped").unwrap();

    // Restart: fresh engine, run initial_scan
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
        IgnoreRules::none(),
    );
    engine.initial_scan().unwrap();

    let results = engine.plan().unwrap();
    let has_upload = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::UploadNew { name, .. }) if name == "offline.txt")
    });
    assert!(has_upload, "initial_scan should pick up file added while stopped");
}
```

- [ ] **Step 2: Run the test, confirm it passes**

```bash
cargo test -q initial_scan_picks_up_file_added_while_stopped
```

Expected: PASS (this scenario already worked; the test documents it).

- [ ] **Step 3: Run fmt and clippy**

```bash
cargo fmt --all
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/sync/tests/sync_tests.rs
git commit -m "test: add unit tests for offline local changes detected on restart"
```

---

### Task 5: Simulation tests

**Files:**
- Modify: `crates/sync/tests/simulator_tests.rs`

- [ ] **Step 1: Add three simulation tests**

Find the section with the existing `simulation_stop_restart_full_cycle_converges` test (around line 688) in `crates/sync/tests/simulator_tests.rs`. Append the following three tests immediately after the `simulation_local_delete_while_stopped_reconciled_on_restart` test (around line 810):

```rust
#[test]
fn simulation_file_added_while_stopped_syncs_on_restart() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    runner.apply(SimAction::StopClient).unwrap();

    // Add a file locally while stopped
    let file_id = LocalFileId::new(1, 9001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_id,
            parent_local_id: Some(root_local_id),
            name: "added_offline.txt".to_string(),
            content: b"added while stopped".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}

#[test]
fn simulation_file_deleted_while_stopped_syncs_on_restart() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-to-delete");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id,
            parent_id: root_id,
            name: "to_delete.txt".to_string(),
            content: b"will be deleted".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Find the local id assigned by sync
    let local_file_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "to_delete.txt")
        .map(|n| n.id.clone())
        .unwrap();

    runner.apply(SimAction::StopClient).unwrap();

    // Delete the file locally while stopped
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: local_file_id,
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}

#[test]
fn simulation_file_replaced_while_stopped_syncs_on_restart() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-to-replace");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id,
            parent_id: root_id,
            name: "replaced.txt".to_string(),
            content: b"original content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Find the local id assigned by sync
    let old_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "replaced.txt")
        .map(|n| n.id.clone())
        .unwrap();
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    runner.apply(SimAction::StopClient).unwrap();

    // Delete old file and create a new one at the same name (different inode)
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: old_local_id,
        })
        .unwrap();
    let new_local_id = LocalFileId::new(1, 9002);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_local_id,
            parent_local_id: Some(root_local_id),
            name: "replaced.txt".to_string(),
            content: b"replacement content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    // Two sync rounds: first uploads the new file, second cleans up the old remote
    runner.apply(SimAction::Sync).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}
```

- [ ] **Step 2: Run the new simulation tests**

```bash
cargo test -q simulation_file_added_while_stopped_syncs_on_restart simulation_file_deleted_while_stopped_syncs_on_restart simulation_file_replaced_while_stopped_syncs_on_restart
```

Expected: all three PASS.

- [ ] **Step 3: Run the full test suite**

```bash
cargo test -q
```

Expected: all tests pass.

- [ ] **Step 4: Run fmt and clippy**

```bash
cargo fmt --all
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/sync/tests/simulator_tests.rs
git commit -m "test: add simulation tests for offline local changes on restart"
```
