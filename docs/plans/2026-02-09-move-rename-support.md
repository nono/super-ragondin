# Move/Rename Support Implementation Plan

**Goal:** Detect and propagate file/directory moves and renames across the 3-tree model (Local, Remote, Synced), including simultaneous move + content changes.

**Architecture:** Add last-synced location metadata (`name`, `parent_id`) to `SyncedRecord` so the planner can determine which side moved. Refactor planner to emit multiple ops per node (move + content change). Fix `TreeStore` children indices for overwrites. Implement `MoveLocal`/`MoveRemote` execution in both the real engine and the simulator.

**Tech Stack:** Rust, fjall, serde, proptest, tempfile

---

### Task 1: Fix `TreeStore` children index on node updates

The `insert_local_node` and `insert_remote_node` methods always insert a new child-key but never remove the old one when overwriting a node whose `(parent_id, name)` changed. This corrupts the children index over time and breaks move detection.

**Files:**
- Modify: `src/store/tree.rs`
- Test: `tests/store_tests.rs`

**Step 1: Write the failing test**

Add a test to `tests/store_tests.rs`:

```rust
#[test]
fn test_update_local_node_removes_old_child_key() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let parent1 = LocalFileId::new(1, 1);
    let parent2 = LocalFileId::new(1, 2);
    let child = LocalFileId::new(1, 100);

    // Insert parent dirs
    let parent1_node = LocalNode {
        id: parent1.clone(),
        parent_id: None,
        name: "dir1".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 1000,
    };
    let parent2_node = LocalNode {
        id: parent2.clone(),
        parent_id: None,
        name: "dir2".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 1000,
    };
    store.insert_local_node(&parent1_node).unwrap();
    store.insert_local_node(&parent2_node).unwrap();

    // Insert child under parent1
    let child_node = LocalNode {
        id: child.clone(),
        parent_id: Some(parent1.clone()),
        name: "file.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(100),
        mtime: 1000,
    };
    store.insert_local_node(&child_node).unwrap();

    // Verify child is listed under parent1
    let children1 = store.list_local_children(&parent1).unwrap();
    assert_eq!(children1.len(), 1);

    // Move child to parent2 by re-inserting with new parent
    let moved_node = LocalNode {
        id: child.clone(),
        parent_id: Some(parent2.clone()),
        name: "renamed.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(100),
        mtime: 1001,
    };
    store.insert_local_node(&moved_node).unwrap();

    // Old parent should have no children
    let children1_after = store.list_local_children(&parent1).unwrap();
    assert_eq!(children1_after.len(), 0, "Old parent should have no children after move");

    // New parent should have the child
    let children2 = store.list_local_children(&parent2).unwrap();
    assert_eq!(children2.len(), 1);
    assert_eq!(children2[0].name, "renamed.txt");
}
```

Also add a symmetric test for remote nodes:

```rust
#[test]
fn test_update_remote_node_removes_old_child_key() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let parent1 = RemoteId::new("parent1");
    let parent2 = RemoteId::new("parent2");

    let parent1_node = RemoteNode {
        id: parent1.clone(),
        parent_id: None,
        name: "dir1".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-a".to_string(),
    };
    let parent2_node = RemoteNode {
        id: parent2.clone(),
        parent_id: None,
        name: "dir2".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-b".to_string(),
    };
    store.insert_remote_node(&parent1_node).unwrap();
    store.insert_remote_node(&parent2_node).unwrap();

    // Insert child under parent1
    let child = RemoteNode {
        id: RemoteId::new("child"),
        parent_id: Some(parent1.clone()),
        name: "file.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: "1-c".to_string(),
    };
    store.insert_remote_node(&child).unwrap();

    let children1 = store.list_remote_children(&parent1).unwrap();
    assert_eq!(children1.len(), 1);

    // Move child to parent2
    let moved = RemoteNode {
        id: RemoteId::new("child"),
        parent_id: Some(parent2.clone()),
        name: "renamed.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(100),
        updated_at: 1001,
        rev: "2-c".to_string(),
    };
    store.insert_remote_node(&moved).unwrap();

    let children1_after = store.list_remote_children(&parent1).unwrap();
    assert_eq!(children1_after.len(), 0, "Old parent should have no children after move");

    let children2 = store.list_remote_children(&parent2).unwrap();
    assert_eq!(children2.len(), 1);
    assert_eq!(children2[0].name, "renamed.txt");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -q test_update_local_node_removes_old_child_key test_update_remote_node_removes_old_child_key`
Expected: FAIL — old parent still lists the child.

**Step 3: Fix `insert_local_node` and `insert_remote_node`**

In `src/store/tree.rs`, update `insert_local_node`:

```rust
pub fn insert_local_node(&self, node: &LocalNode) -> Result<()> {
    // Remove old child-key if node already exists with different parent/name
    if let Some(old_node) = self.get_local_node(&node.id)? {
        if let Some(old_parent) = &old_node.parent_id {
            let old_child_key = make_child_key_local(old_parent, &old_node.name);
            self.local_children.remove(old_child_key)?;
        }
    }

    let key = node.id.to_bytes();
    let value = serde_json::to_vec(node)?;
    self.local.insert(key, value)?;

    if let Some(parent_id) = &node.parent_id {
        let child_key = make_child_key_local(parent_id, &node.name);
        self.local_children.insert(child_key, key)?;
    }
    Ok(())
}
```

Apply the same pattern to `insert_remote_node`:

```rust
pub fn insert_remote_node(&self, node: &RemoteNode) -> Result<()> {
    // Remove old child-key if node already exists with different parent/name
    if let Some(old_node) = self.get_remote_node(&node.id)? {
        if let Some(old_parent) = &old_node.parent_id {
            let old_child_key = make_child_key_remote(old_parent, &old_node.name);
            self.remote_children.remove(old_child_key)?;
        }
    }

    let key = node.id.as_str().as_bytes();
    let value = serde_json::to_vec(node)?;
    self.remote.insert(key, value)?;

    if let Some(parent_id) = &node.parent_id {
        let child_key = make_child_key_remote(parent_id, &node.name);
        self.remote_children
            .insert(child_key, node.id.as_str().as_bytes())?;
    }
    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -q test_update_local_node_removes_old_child_key test_update_remote_node_removes_old_child_key`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test -q`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add src/store/tree.rs tests/store_tests.rs
git commit -m "fix: remove stale children index entries on node update"
```

---

### Task 2: Add location fields to `SyncedRecord`

Add `name` and `parent_id` for both sides to `SyncedRecord`, so the planner can detect which side moved since last sync. Use `Option` + `#[serde(default)]` for backward compatibility.

**Files:**
- Modify: `src/model.rs`
- Test: `src/model.rs` (unit tests)

**Step 1: Write the failing test**

Add to `src/model.rs` tests:

```rust
#[test]
fn synced_record_with_location_fields() {
    let record = SyncedRecord {
        local_id: LocalFileId::new(1, 100),
        remote_id: RemoteId::new("remote-123"),
        rel_path: "docs/file.txt".to_string(),
        md5sum: Some("abc123".to_string()),
        size: Some(1024),
        rev: "1-xyz".to_string(),
        node_type: NodeType::File,
        local_name: Some("file.txt".to_string()),
        local_parent_id: Some(LocalFileId::new(1, 50)),
        remote_name: Some("file.txt".to_string()),
        remote_parent_id: Some(RemoteId::new("docs-dir")),
    };

    let json = serde_json::to_string(&record).unwrap();
    let deserialized: SyncedRecord = serde_json::from_str(&json).unwrap();

    assert_eq!(record, deserialized);
    assert_eq!(deserialized.local_name, Some("file.txt".to_string()));
    assert_eq!(deserialized.remote_parent_id, Some(RemoteId::new("docs-dir")));
}

#[test]
fn synced_record_backward_compatible_deserialization() {
    // Simulate a record serialized before the location fields existed
    let json = r#"{
        "local_id": {"device_id": 1, "inode": 100},
        "remote_id": "remote-123",
        "rel_path": "file.txt",
        "md5sum": "abc",
        "size": 100,
        "rev": "1-x",
        "node_type": "file"
    }"#;

    let record: SyncedRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.local_name, None);
    assert_eq!(record.local_parent_id, None);
    assert_eq!(record.remote_name, None);
    assert_eq!(record.remote_parent_id, None);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -q synced_record_with_location_fields synced_record_backward_compatible`
Expected: FAIL — fields don't exist yet.

**Step 3: Add fields to `SyncedRecord`**

In `src/model.rs`, add to `SyncedRecord`:

```rust
pub struct SyncedRecord {
    pub local_id: LocalFileId,
    pub remote_id: RemoteId,
    pub rel_path: String,
    pub md5sum: Option<String>,
    pub size: Option<u64>,
    pub rev: String,
    pub node_type: NodeType,
    #[serde(default)]
    pub local_name: Option<String>,
    #[serde(default)]
    pub local_parent_id: Option<LocalFileId>,
    #[serde(default)]
    pub remote_name: Option<String>,
    #[serde(default)]
    pub remote_parent_id: Option<RemoteId>,
}
```

**Step 4: Fix all compilation errors**

Every place that constructs a `SyncedRecord` must now include the 4 new fields. Update all call sites:

- `src/sync/engine.rs` — `execute_create_local_dir`: add `local_name`, `local_parent_id`, `remote_name`, `remote_parent_id` from the `remote_node` and `local_node`.
- `src/simulator/runner.rs` — every `SyncedRecord { .. }` construction (there are ~5): add the 4 fields from the local/remote node being synced.
- `tests/planner_tests.rs` — `make_synced` helper: add params or default to `None`.
- `tests/sync_tests.rs` — if any inline `SyncedRecord` construction exists.

For `make_synced` in planner tests, the simplest approach is to add optional parameters with defaults:

```rust
fn make_synced(
    local_id: LocalFileId,
    remote_id: RemoteId,
    path: &str,
    md5: Option<&str>,
    node_type: NodeType,
) -> SyncedRecord {
    SyncedRecord {
        local_id,
        remote_id,
        rel_path: path.to_string(),
        md5sum: md5.map(String::from),
        size: Some(100),
        rev: "1-abc".to_string(),
        node_type,
        local_name: None,
        local_parent_id: None,
        remote_name: None,
        remote_parent_id: None,
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -q`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add src/model.rs src/sync/engine.rs src/simulator/runner.rs tests/planner_tests.rs tests/sync_tests.rs
git commit -m "feat: add location fields to SyncedRecord for move detection"
```

---

### Task 3: Add `make_synced_with_location` helper for planner tests

Create a helper that sets all location fields, used by the new move-detection tests.

**Files:**
- Modify: `tests/planner_tests.rs`

**Step 1: Add the helper**

```rust
fn make_synced_with_location(
    local_id: LocalFileId,
    remote_id: RemoteId,
    path: &str,
    md5: Option<&str>,
    node_type: NodeType,
    local_name: &str,
    local_parent_id: Option<LocalFileId>,
    remote_name: &str,
    remote_parent_id: Option<RemoteId>,
) -> SyncedRecord {
    SyncedRecord {
        local_id,
        remote_id,
        rel_path: path.to_string(),
        md5sum: md5.map(String::from),
        size: Some(100),
        rev: "1-abc".to_string(),
        node_type,
        local_name: Some(local_name.to_string()),
        local_parent_id,
        remote_name: Some(remote_name.to_string()),
        remote_parent_id,
    }
}
```

**Step 2: Run tests**

Run: `cargo test -q`
Expected: PASS (helper is unused yet, but compiles).

**Step 3: Commit**

```bash
git add tests/planner_tests.rs
git commit -m "test: add make_synced_with_location helper for move tests"
```

---

### Task 4: Add `BothMoved` conflict kind

**Files:**
- Modify: `src/model.rs`

**Step 1: Add variant**

Add `BothMoved` to `ConflictKind`:

```rust
pub enum ConflictKind {
    BothModified,
    LocalDeleteRemoteModify,
    LocalModifyRemoteDelete,
    ParentMissing,
    NameCollision,
    BothMoved,
}
```

**Step 2: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 3: Commit**

```bash
git add src/model.rs
git commit -m "feat: add BothMoved conflict kind"
```

---

### Task 5: Refactor planner to return `Vec<PlanResult>` per node

A node might need both a move op and a content op in the same cycle. Refactor planner internals to produce `Vec<PlanResult>` instead of `Option<PlanResult>`.

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_tests.rs`

**Step 1: Run existing tests to establish baseline**

Run: `cargo test -q`
Expected: PASS.

**Step 2: Refactor internal methods**

Change signatures in `src/planner.rs`:

- `plan_remote_node(...)` → returns `Vec<PlanResult>` (was `Option<PlanResult>`)
- `plan_all_three(...)` → returns `Vec<PlanResult>` (was `Option<PlanResult>`)
- `plan_remote_only(...)` → returns `Vec<PlanResult>` (was `Option<PlanResult>`)
- `plan_created_both_sides(...)` → returns `Vec<PlanResult>` (was `Option<PlanResult>`)
- `plan_local_only(...)` → returns `Vec<PlanResult>` (was `Option<PlanResult>`)

In `plan()`, change from:

```rust
if let Some(result) = self.plan_remote_node(...) {
    results.push(result);
}
```

to:

```rust
results.extend(self.plan_remote_node(...));
```

And similarly for `plan_local_only`.

Inside each method, convert:
- `None` → `vec![]`
- `Some(x)` → `vec![x]`

**Step 3: Run tests to verify refactor is behavior-preserving**

Run: `cargo test -q`
Expected: All existing tests PASS.

**Step 4: Commit**

```bash
git add src/planner.rs
git commit -m "refactor: planner returns Vec<PlanResult> per node for multi-op support"
```

---

### Task 6: Detect remote-only moves and plan `MoveLocal`

When the remote side has changed `name` or `parent_id` since last sync but local hasn't, plan a `MoveLocal` operation.

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_tests.rs`

**Step 1: Write failing test — remote rename (same parent)**

```rust
#[test]
fn test_remote_rename_generates_move_local() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    // Set up parent binding
    let synced_root = make_synced_with_location(
        parent_lid.clone(), parent_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();

    // Parent nodes
    let parent_local = make_local_dir(parent_lid.clone(), None, "");
    let parent_remote = make_remote_dir(parent_rid.clone(), None, "");
    store.insert_local_node(&parent_local).unwrap();
    store.insert_remote_node(&parent_remote).unwrap();

    // Synced record with location info: name was "old.txt"
    let synced = make_synced_with_location(
        lid.clone(), rid.clone(),
        "old.txt", Some("hash"), NodeType::File,
        "old.txt", Some(parent_lid.clone()),
        "old.txt", Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Local still has old name
    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "old.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    // Remote has new name (renamed)
    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    let move_op = ops.iter().find(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    assert!(move_op.is_some(), "Should plan MoveLocal for remote rename");

    if let Some(PlanResult::Op(SyncOp::MoveLocal {
        local_id: op_lid,
        from_path,
        to_path,
        ..
    })) = move_op
    {
        assert_eq!(*op_lid, lid);
        assert!(from_path.to_string_lossy().contains("old.txt"));
        assert!(to_path.to_string_lossy().contains("new.txt"));
    }
}
```

**Step 2: Write failing test — remote move to different directory**

```rust
#[test]
fn test_remote_move_to_different_dir_generates_move_local() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent1_lid = local_id(1, 1);
    let parent1_rid = remote_id("dir1");
    let parent2_lid = local_id(1, 2);
    let parent2_rid = remote_id("dir2");
    let root_lid = local_id(1, 0);
    let root_rid = remote_id("root");

    // Root binding
    let synced_root = make_synced_with_location(
        root_lid.clone(), root_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();
    let root_local = make_local_dir(root_lid.clone(), None, "");
    let root_remote = make_remote_dir(root_rid.clone(), None, "");
    store.insert_local_node(&root_local).unwrap();
    store.insert_remote_node(&root_remote).unwrap();

    // Parent1 binding
    let synced_p1 = make_synced_with_location(
        parent1_lid.clone(), parent1_rid.clone(),
        "dir1", None, NodeType::Directory,
        "dir1", Some(root_lid.clone()),
        "dir1", Some(root_rid.clone()),
    );
    store.insert_synced(&synced_p1).unwrap();
    let p1_local = make_local_dir(parent1_lid.clone(), Some(root_lid.clone()), "dir1");
    let p1_remote = make_remote_dir(parent1_rid.clone(), Some(root_rid.clone()), "dir1");
    store.insert_local_node(&p1_local).unwrap();
    store.insert_remote_node(&p1_remote).unwrap();

    // Parent2 binding
    let synced_p2 = make_synced_with_location(
        parent2_lid.clone(), parent2_rid.clone(),
        "dir2", None, NodeType::Directory,
        "dir2", Some(root_lid.clone()),
        "dir2", Some(root_rid.clone()),
    );
    store.insert_synced(&synced_p2).unwrap();
    let p2_local = make_local_dir(parent2_lid.clone(), Some(root_lid.clone()), "dir2");
    let p2_remote = make_remote_dir(parent2_rid.clone(), Some(root_rid.clone()), "dir2");
    store.insert_local_node(&p2_local).unwrap();
    store.insert_remote_node(&p2_remote).unwrap();

    // File synced under parent1
    let synced_file = make_synced_with_location(
        lid.clone(), rid.clone(),
        "dir1/file.txt", Some("hash"), NodeType::File,
        "file.txt", Some(parent1_lid.clone()),
        "file.txt", Some(parent1_rid.clone()),
    );
    store.insert_synced(&synced_file).unwrap();

    // Local still under parent1
    let local_file = make_local_file(lid.clone(), Some(parent1_lid.clone()), "file.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    // Remote moved to parent2
    let remote_file = make_remote_file(rid.clone(), Some(parent2_rid.clone()), "file.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    let move_op = ops.iter().find(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    assert!(move_op.is_some(), "Should plan MoveLocal for remote directory move");

    if let Some(PlanResult::Op(SyncOp::MoveLocal { to_path, .. })) = move_op {
        assert!(to_path.to_string_lossy().contains("dir2"), "Should move to dir2");
    }
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -q test_remote_rename_generates_move_local test_remote_move_to_different_dir`
Expected: FAIL.

**Step 4: Implement move detection in `plan_all_three`**

In `src/planner.rs`, modify `plan_all_three` to detect location changes. Add helper methods:

```rust
fn remote_location_changed(remote: &RemoteNode, synced: &SyncedRecord) -> bool {
    let Some(synced_name) = &synced.remote_name else { return false };
    let Some(synced_parent) = &synced.remote_parent_id else {
        return synced_name.as_str() != remote.name;
    };
    synced_name.as_str() != remote.name
        || remote.parent_id.as_ref() != Some(synced_parent)
}

fn local_location_changed(local: &LocalNode, synced: &SyncedRecord) -> bool {
    let Some(synced_name) = &synced.local_name else { return false };
    let Some(synced_parent) = &synced.local_parent_id else {
        return synced_name.as_str() != local.name;
    };
    synced_name.as_str() != local.name
        || local.parent_id.as_ref() != Some(synced_parent)
}
```

In `plan_all_three`, compute location flags alongside content flags, and emit `MoveLocal`/`MoveRemote` ops:

- If `remote_loc_changed && !local_loc_changed`: resolve the target local parent via `store.get_synced_by_remote(remote.parent_id)`, then emit `SyncOp::MoveLocal`.
- Content ops are handled independently (can emit both move + content op).

**Step 5: Run tests to verify they pass**

Run: `cargo test -q`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add src/planner.rs tests/planner_tests.rs
git commit -m "feat: detect remote moves and plan MoveLocal operations"
```

---

### Task 7: Detect local-only moves and plan `MoveRemote`

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_tests.rs`

**Step 1: Write failing test — local rename**

```rust
#[test]
fn test_local_rename_generates_move_remote() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    // Parent binding
    let synced_root = make_synced_with_location(
        parent_lid.clone(), parent_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();
    let parent_local = make_local_dir(parent_lid.clone(), None, "");
    let parent_remote = make_remote_dir(parent_rid.clone(), None, "");
    store.insert_local_node(&parent_local).unwrap();
    store.insert_remote_node(&parent_remote).unwrap();

    // File synced as "old.txt"
    let synced = make_synced_with_location(
        lid.clone(), rid.clone(),
        "old.txt", Some("hash"), NodeType::File,
        "old.txt", Some(parent_lid.clone()),
        "old.txt", Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Local renamed to "new.txt"
    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "new.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    // Remote still has old name
    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "old.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    let move_op = ops.iter().find(|r| matches!(r, PlanResult::Op(SyncOp::MoveRemote { .. })));
    assert!(move_op.is_some(), "Should plan MoveRemote for local rename");

    if let Some(PlanResult::Op(SyncOp::MoveRemote {
        remote_id: op_rid,
        new_name,
        ..
    })) = move_op
    {
        assert_eq!(op_rid.as_str(), "f1");
        assert_eq!(new_name, "new.txt");
    }
}
```

**Step 2: Run tests to verify it fails**

Run: `cargo test -q test_local_rename_generates_move_remote`
Expected: FAIL.

**Step 3: Implement local move detection**

In `plan_all_three`, add the symmetric case: if `local_loc_changed && !remote_loc_changed`, resolve the target remote parent via `store.get_synced_by_local(local.parent_id)`, then emit `SyncOp::MoveRemote`.

**Step 4: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/planner.rs tests/planner_tests.rs
git commit -m "feat: detect local moves and plan MoveRemote operations"
```

---

### Task 8: Handle both-sides-moved scenarios

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_tests.rs`

**Step 1: Write failing test — both moved to same location (no-op)**

```rust
#[test]
fn test_both_moved_to_same_location_is_noop() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(), parent_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();
    store.insert_local_node(&make_local_dir(parent_lid.clone(), None, "")).unwrap();
    store.insert_remote_node(&make_remote_dir(parent_rid.clone(), None, "")).unwrap();

    // Synced as "old.txt"
    let synced = make_synced_with_location(
        lid.clone(), rid.clone(),
        "old.txt", Some("hash"), NodeType::File,
        "old.txt", Some(parent_lid.clone()),
        "old.txt", Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Both renamed to "new.txt"
    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "new.txt", "hash");
    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "hash");
    store.insert_local_node(&local_file).unwrap();
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    // No move ops needed since both sides converged
    let move_ops: Vec<_> = ops.iter().filter(|r| matches!(r,
        PlanResult::Op(SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. })
    )).collect();
    assert!(move_ops.is_empty(), "No move ops when both sides moved to same location");
}
```

**Step 2: Write failing test — both moved to different locations (conflict)**

```rust
#[test]
fn test_both_moved_to_different_locations_is_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(), parent_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();
    store.insert_local_node(&make_local_dir(parent_lid.clone(), None, "")).unwrap();
    store.insert_remote_node(&make_remote_dir(parent_rid.clone(), None, "")).unwrap();

    let synced = make_synced_with_location(
        lid.clone(), rid.clone(),
        "old.txt", Some("hash"), NodeType::File,
        "old.txt", Some(parent_lid.clone()),
        "old.txt", Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Local renamed to "local_name.txt"
    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "local_name.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    // Remote renamed to "remote_name.txt"
    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "remote_name.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    let conflict = ops.iter().find(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::BothMoved));
    assert!(conflict.is_some(), "Should produce BothMoved conflict");
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -q test_both_moved_to_same_location test_both_moved_to_different_locations`
Expected: FAIL.

**Step 4: Implement both-moved logic**

In `plan_all_three`, when `remote_loc_changed && local_loc_changed`:
- Check if local and remote now point to the same location (same name, and local's parent maps to remote's parent via synced binding). If so, no move op needed.
- Otherwise, emit `PlanResult::Conflict` with `ConflictKind::BothMoved`.

**Step 5: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 6: Commit**

```bash
git add src/planner.rs tests/planner_tests.rs
git commit -m "feat: handle both-moved scenarios (no-op or conflict)"
```

---

### Task 9: Handle move + content change simultaneously

**Files:**
- Modify: `src/planner.rs`
- Test: `tests/planner_tests.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_remote_rename_and_content_change_generates_move_and_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(), parent_rid.clone(),
        "", None, NodeType::Directory,
        "", None, "", None,
    );
    store.insert_synced(&synced_root).unwrap();
    store.insert_local_node(&make_local_dir(parent_lid.clone(), None, "")).unwrap();
    store.insert_remote_node(&make_remote_dir(parent_rid.clone(), None, "")).unwrap();

    let synced = make_synced_with_location(
        lid.clone(), rid.clone(),
        "old.txt", Some("old_hash"), NodeType::File,
        "old.txt", Some(parent_lid.clone()),
        "old.txt", Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Local unchanged
    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "old.txt", "old_hash");
    store.insert_local_node(&local_file).unwrap();

    // Remote renamed AND content changed
    let mut remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "new_hash");
    remote_file.rev = "2-abc".to_string();
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    let has_move = ops.iter().any(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    let has_download = ops.iter().any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadUpdate { .. })));

    assert!(has_move, "Should plan MoveLocal");
    assert!(has_download, "Should plan DownloadUpdate");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -q test_remote_rename_and_content_change`
Expected: FAIL.

**Step 3: Implement**

This should largely work from the refactoring in Tasks 5-6, since `plan_all_three` now returns `Vec<PlanResult>` and can push both a move op and a content op. Verify the move op uses the *new* path (post-move) for the `DownloadUpdate` `local_path`.

**Step 4: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/planner.rs tests/planner_tests.rs
git commit -m "feat: support simultaneous move + content change"
```

---

### Task 10: Update operation sort order — moves before transfers

Moves must execute before content transfers so that paths are correct.

**Files:**
- Modify: `src/planner.rs`

**Step 1: Update `sort_operations`**

```rust
fn sort_operations(results: &mut [PlanResult]) {
    results.sort_by_key(|r| match r {
        PlanResult::Op(SyncOp::CreateLocalDir { .. } | SyncOp::CreateRemoteDir { .. }) => 0,
        PlanResult::Op(SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. }) => 1,
        PlanResult::Op(
            SyncOp::DownloadNew { .. }
            | SyncOp::DownloadUpdate { .. }
            | SyncOp::UploadNew { .. }
            | SyncOp::UploadUpdate { .. },
        ) => 2,
        PlanResult::Op(SyncOp::DeleteLocal { .. } | SyncOp::DeleteRemote { .. }) => 3,
        PlanResult::Conflict(_) => 4,
        PlanResult::NoOp => 5,
    });
}
```

**Step 2: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 3: Commit**

```bash
git add src/planner.rs
git commit -m "fix: sort moves before transfers in operation ordering"
```

---

### Task 11: Implement `MoveLocal` execution in `SyncEngine`

**Files:**
- Modify: `src/sync/engine.rs`
- Test: `tests/sync_tests.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_sync_engine_execute_move_local() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp, SyncedRecord};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    // Create source file
    let from_path = sync_dir.path().join("old.txt");
    fs::write(&from_path, "content").unwrap();

    let metadata = fs::metadata(&from_path).unwrap();
    let local_id = LocalFileId::new(metadata.dev(), metadata.ino());
    let parent_id = {
        let m = fs::metadata(sync_dir.path()).unwrap();
        LocalFileId::new(m.dev(), m.ino())
    };

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Insert local node
    let local_node = LocalNode {
        id: local_id.clone(),
        parent_id: Some(parent_id.clone()),
        name: "old.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(7),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    let to_path = sync_dir.path().join("new.txt");
    let op = SyncOp::MoveLocal {
        local_id: local_id.clone(),
        from_path: from_path.clone(),
        to_path: to_path.clone(),
        expected_parent_id: parent_id.clone(),
        expected_name: "old.txt".to_string(),
    };

    engine.execute_op(&op).unwrap();

    assert!(!from_path.exists(), "Old path should not exist");
    assert!(to_path.exists(), "New path should exist");
    assert_eq!(fs::read_to_string(&to_path).unwrap(), "content");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -q test_sync_engine_execute_move_local`
Expected: FAIL (currently a no-op stub).

**Step 3: Implement `execute_move_local`**

In `src/sync/engine.rs`, add a handler in `execute_op` for `MoveLocal`:

```rust
SyncOp::MoveLocal {
    local_id,
    from_path,
    to_path,
    expected_parent_id: _,
    expected_name: _,
} => self.execute_move_local(local_id, from_path, to_path),
```

```rust
fn execute_move_local(
    &self,
    local_id: &LocalFileId,
    from_path: &Path,
    to_path: &Path,
) -> Result<()> {
    if let Some(parent) = to_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from_path, to_path)?;

    // Update local node in store
    if let Some(mut node) = self.store.get_local_node(local_id)? {
        node.name = to_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // Update parent_id from new path's parent metadata
        if let Some(parent_path) = to_path.parent() {
            let parent_meta = fs::metadata(parent_path)?;
            node.parent_id = Some(LocalFileId::new(parent_meta.dev(), parent_meta.ino()));
        }
        self.store.insert_local_node(&node)?;
    }

    self.store.flush()?;
    Ok(())
}
```

**Step 4: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/sync/engine.rs tests/sync_tests.rs
git commit -m "feat: implement MoveLocal execution in SyncEngine"
```

---

### Task 12: Add `SimAction` variants for moves in the simulator

**Files:**
- Modify: `src/simulator/runner.rs`
- Modify: `src/simulator/mock_fs.rs`
- Modify: `src/simulator/mock_remote.rs`
- Test: `tests/simulator_tests.rs`

**Step 1: Write failing test**

```rust
#[test]
fn simulation_runner_remote_rename_then_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner.apply(SimAction::RemoteCreateDir {
        id: root_id.clone(),
        parent_id: None,
        name: String::new(),
    }).unwrap();

    // Create file on remote
    let file_id = RemoteId::new("file-1");
    runner.apply(SimAction::RemoteCreateFile {
        id: file_id.clone(),
        parent_id: root_id.clone(),
        name: "old.txt".to_string(),
        content: b"hello".to_vec(),
    }).unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // Verify file exists locally
    let local_files: Vec<_> = runner.local_fs.list_all().into_iter()
        .filter(|n| n.name == "old.txt").collect();
    assert_eq!(local_files.len(), 1);

    // Rename file on remote
    runner.apply(SimAction::RemoteMove {
        id: file_id.clone(),
        new_parent_id: root_id.clone(),
        new_name: "new.txt".to_string(),
    }).unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // File should be renamed locally
    let old_files: Vec<_> = runner.local_fs.list_all().into_iter()
        .filter(|n| n.name == "old.txt").collect();
    assert_eq!(old_files.len(), 0, "Old name should be gone");

    let new_files: Vec<_> = runner.local_fs.list_all().into_iter()
        .filter(|n| n.name == "new.txt").collect();
    assert_eq!(new_files.len(), 1, "New name should exist");

    runner.check_convergence().unwrap();
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -q simulation_runner_remote_rename_then_sync`
Expected: FAIL — `SimAction::RemoteMove` doesn't exist.

**Step 3: Add `SimAction::LocalMove` and `SimAction::RemoteMove`**

In `src/simulator/runner.rs`, add to `SimAction`:

```rust
LocalMove {
    local_id: LocalFileId,
    new_parent_local_id: Option<LocalFileId>,
    new_name: String,
},
RemoteMove {
    id: RemoteId,
    new_parent_id: RemoteId,
    new_name: String,
},
```

Add `MockFs::move_node` in `mock_fs.rs`:

```rust
pub fn move_node(&mut self, id: &LocalFileId, new_parent: Option<LocalFileId>, new_name: String) {
    if let Some(node) = self.nodes.get_mut(id) {
        node.parent_id = new_parent;
        node.name = new_name;
    }
}
```

Add `MockRemote::move_node` in `mock_remote.rs`:

```rust
pub fn move_node(&mut self, id: &RemoteId, new_parent_id: RemoteId, new_name: String) {
    if let Some(node) = self.nodes.get_mut(id) {
        node.parent_id = Some(new_parent_id);
        node.name = new_name;
        let rev_num: u32 = node.rev.split('-').next()
            .and_then(|s| s.parse().ok()).unwrap_or(1);
        node.rev = format!("{}-{}", rev_num + 1, id.as_str());
    }
    self.seq += 1;
    self.changes.push(ChangeRecord {
        seq: self.seq,
        remote_id: id.clone(),
        deleted: false,
    });
}
```

Implement `apply` handlers for both new `SimAction` variants. Also implement `SyncOp::MoveLocal` and `SyncOp::MoveRemote` in `execute_op`.

**Step 4: Populate location fields in `SyncedRecord` everywhere in the simulator**

Update every `SyncedRecord` construction in `runner.rs` to include `local_name`, `local_parent_id`, `remote_name`, `remote_parent_id` from the node data.

**Step 5: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 6: Commit**

```bash
git add src/simulator/ tests/simulator_tests.rs
git commit -m "feat: add move/rename support to simulator"
```

---

### Task 13: Add property-based test for move convergence

**Files:**
- Modify: `tests/simulator_tests.rs`

**Step 1: Write proptest**

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    #[test]
    fn prop_remote_rename_then_sync_converges(
        original_name in arbitrary_file_name(),
        new_name in arbitrary_file_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        }).unwrap();

        let file_id = RemoteId::new("file-1");
        runner.apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: original_name,
            content,
        }).unwrap();

        runner.apply(SimAction::Sync).unwrap();

        runner.apply(SimAction::RemoteMove {
            id: file_id,
            new_parent_id: root_id,
            new_name,
        }).unwrap();

        runner.apply(SimAction::Sync).unwrap();

        runner.check_convergence().unwrap();
    }
}
```

**Step 2: Run test**

Run: `cargo test -q prop_remote_rename_then_sync_converges`
Expected: PASS.

**Step 3: Commit**

```bash
git add tests/simulator_tests.rs
git commit -m "test: add property-based test for move convergence"
```

---

### Task 14: Fix `plan_remote_deleted` to use computed path instead of `rel_path`

After moves, `synced.rel_path` may be stale. Use `compute_local_path_from_local` instead.

**Files:**
- Modify: `src/planner.rs`

**Step 1: Fix the code**

In `plan_remote_deleted`, change:

```rust
local_path: self.sync_root.join(&synced.rel_path),
```

to:

```rust
local_path: self.compute_local_path_from_local(local),
```

**Step 2: Run tests**

Run: `cargo test -q`
Expected: PASS.

**Step 3: Commit**

```bash
git add src/planner.rs
git commit -m "fix: use computed path instead of stale rel_path for delete operations"
```

---

## Summary

| Task | Description | Effort |
|------|-------------|--------|
| 1 | Fix `TreeStore` children index on node updates | S |
| 2 | Add location fields to `SyncedRecord` | M |
| 3 | Add `make_synced_with_location` test helper | S |
| 4 | Add `BothMoved` conflict kind | S |
| 5 | Refactor planner to return `Vec<PlanResult>` per node | M |
| 6 | Detect remote-only moves → plan `MoveLocal` | L |
| 7 | Detect local-only moves → plan `MoveRemote` | M |
| 8 | Handle both-sides-moved scenarios | M |
| 9 | Handle move + content change simultaneously | M |
| 10 | Update operation sort order (moves before transfers) | S |
| 11 | Implement `MoveLocal` execution in `SyncEngine` | M |
| 12 | Add move support to simulator | L |
| 13 | Property-based test for move convergence | S |
| 14 | Fix `plan_remote_deleted` to use computed path | S |
