# Changes Feed Conformance Tests

**Goal:** Verify that the simulator's `MockRemote` changes feed behaves identically to the real cozy-stack changes feed, by replaying the same action sequences against both and comparing results.

**Architecture:** Define a set of "remote action" scenarios (sequences of create/move/rename/trash operations). Each scenario is played against a real cozy-stack instance via `CozyClient`, then against `MockRemote`. After each scenario, we compare: (1) the final tree state (names, parent relationships, types), and (2) the changes feed structure (which nodes appear as changed, whether they're marked deleted, ordering guarantees). The comparison ignores fields that are inherently different (real IDs vs synthetic IDs, timestamps, opaque seq strings vs u64), and focuses on structural equivalence.

**Tech Stack:** Rust, tokio, cozy-stack CLI, `CozyClient`, `MockRemote`

---

## Background: Known Behavioral Questions

Before implementing, these are the key behavioral questions the conformance tests should answer:

1. **Move/rename a directory:** Does the cozy-stack changes feed emit a change only for the moved directory, or also for all its children? The `MockRemote.move_node` only records a change for the moved node — is this accurate?
2. **Trash a directory with children:** Does cozy-stack emit individual change records for each child, or only for the trashed parent? The `MockRemote` currently records a move for the parent only via `move_node`.
3. **Delete vs trash:** How do the changes feeds differ for hard-delete (`DELETE /files/:id?permanently`) vs trash (`DELETE /files/:id`)?
4. **Create file inside new directory:** When a directory and then a file inside it are created in sequence, are the changes always ordered parent-before-child?
5. **Multiple modifications:** When a node is modified multiple times between two `fetch_changes` calls, does the changes feed coalesce them into one entry or report multiple?

---

### Task 1: Define the `RemoteAction` enum and scenario builder

**Files:**
- Create: `tests/changes_feed_conformance.rs`

**Step 1: Create the test file with the action enum and scenario type**

Define a small DSL of remote-only actions that can be replayed against both backends. These are intentionally simpler than `SimAction` — they only model remote mutations (what another Cozy client would do).

```rust
//! Conformance tests: verify MockRemote changes feed matches real cozy-stack.
//!
//! Run with:
//! ```bash
//! cargo test --test changes_feed_conformance -- --ignored
//! ```

use cozy_desktop::model::{NodeType, RemoteId, RemoteNode};
use cozy_desktop::remote::client::CozyClient;
use cozy_desktop::simulator::mock_remote::MockRemote;
use cozy_desktop::util::compute_md5_from_bytes;
use std::process::Command;
use tempfile::TempDir;

/// A remote mutation that can be replayed against both MockRemote and CozyClient.
#[derive(Debug, Clone)]
enum RemoteAction {
    CreateDir {
        /// Logical name used to reference this node in later actions
        ref_name: String,
        /// Logical name of the parent (None = root)
        parent_ref: Option<String>,
        name: String,
    },
    CreateFile {
        ref_name: String,
        parent_ref: Option<String>,
        name: String,
        content: Vec<u8>,
    },
    Rename {
        ref_name: String,
        new_name: String,
    },
    Move {
        ref_name: String,
        new_parent_ref: String,
    },
    MoveAndRename {
        ref_name: String,
        new_parent_ref: String,
        new_name: String,
    },
    Trash {
        ref_name: String,
    },
}

/// A named scenario: a sequence of actions with a description.
struct Scenario {
    name: &'static str,
    actions: Vec<RemoteAction>,
}
```

**Step 2: Run `cargo check --test changes_feed_conformance` to verify it compiles**

Expected: compiles (all types are unused but that's OK for now)

**Step 3: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: scaffold changes feed conformance test with RemoteAction DSL"
```

---

### Task 2: Implement the `MockRemote` replayer

**Files:**
- Modify: `tests/changes_feed_conformance.rs`

**Step 1: Add the mock replayer**

This replays a `Vec<RemoteAction>` against a `MockRemote`, tracking `ref_name → RemoteId` mappings. After replay, it records a snapshot of the changes feed and final tree state.

```rust
use std::collections::HashMap;

/// Normalized representation of a single change entry for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedChange {
    /// Logical ref_name (mapped back from ID)
    ref_name: String,
    /// Whether this is a deletion
    deleted: bool,
    /// For non-deleted: the node's name at this point
    name: Option<String>,
    /// For non-deleted: the parent's ref_name (None = root)
    parent_ref: Option<String>,
    /// File or Directory
    node_type: Option<NodeType>,
}

/// Normalized tree state for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedNode {
    ref_name: String,
    name: String,
    parent_ref: Option<String>,
    node_type: NodeType,
    md5sum: Option<String>,
}

/// Result of replaying a scenario against a backend.
#[derive(Debug)]
struct ReplayResult {
    /// Changes recorded during replay (ordered)
    changes: Vec<NormalizedChange>,
    /// Final tree state (unordered)
    final_tree: Vec<NormalizedNode>,
}

fn replay_on_mock(actions: &[RemoteAction]) -> ReplayResult {
    let mut remote = MockRemote::new();
    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let mut id_counter = 0u32;

    // Create root
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    let root_node = RemoteNode {
        id: root_id.clone(),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-root".to_string(),
    };
    remote.add_node(root_node, None);
    let seq_after_root = remote.current_seq();

    for action in actions {
        match action {
            RemoteAction::CreateDir { ref_name, parent_ref, name } => {
                id_counter += 1;
                let id = RemoteId::new(format!("mock-{id_counter}"));
                let parent_id = parent_ref.as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .cloned()
                    .unwrap_or_else(|| root_id.clone());
                let node = RemoteNode {
                    id: id.clone(),
                    parent_id: Some(parent_id),
                    name: name.clone(),
                    node_type: NodeType::Directory,
                    md5sum: None,
                    size: None,
                    updated_at: 1000,
                    rev: format!("1-{}", id.as_str()),
                };
                remote.add_node(node, None);
                ref_to_id.insert(ref_name.clone(), id.clone());
                id_to_ref.insert(id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::CreateFile { ref_name, parent_ref, name, content } => {
                id_counter += 1;
                let id = RemoteId::new(format!("mock-{id_counter}"));
                let parent_id = parent_ref.as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .cloned()
                    .unwrap_or_else(|| root_id.clone());
                let md5 = compute_md5_from_bytes(content);
                let node = RemoteNode {
                    id: id.clone(),
                    parent_id: Some(parent_id),
                    name: name.clone(),
                    node_type: NodeType::File,
                    md5sum: Some(md5),
                    size: Some(content.len() as u64),
                    updated_at: 1000,
                    rev: format!("1-{}", id.as_str()),
                };
                remote.add_node(node, Some(content.clone()));
                ref_to_id.insert(ref_name.clone(), id.clone());
                id_to_ref.insert(id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::Rename { ref_name, new_name } => {
                let id = &ref_to_id[ref_name];
                let parent_id = remote.get_node(id).unwrap().parent_id.clone().unwrap();
                remote.move_node(id, parent_id, new_name.clone());
            }
            RemoteAction::Move { ref_name, new_parent_ref } => {
                let id = &ref_to_id[ref_name];
                let name = remote.get_node(id).unwrap().name.clone();
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, name);
            }
            RemoteAction::MoveAndRename { ref_name, new_parent_ref, new_name } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, new_name.clone());
            }
            RemoteAction::Trash { ref_name } => {
                let id = &ref_to_id[ref_name];
                // MockRemote doesn't have a built-in trash, simulate via delete
                remote.delete_node(id);
            }
        }
    }

    // Collect changes (skip root creation)
    let changes = remote.get_all_changes_since(seq_after_root)
        .into_iter()
        .map(|c| {
            let ref_name = id_to_ref.get(c.remote_id.as_str())
                .cloned()
                .unwrap_or_else(|| c.remote_id.as_str().to_string());
            if c.deleted {
                NormalizedChange {
                    ref_name,
                    deleted: true,
                    name: None,
                    parent_ref: None,
                    node_type: None,
                }
            } else {
                let node = remote.get_node(&c.remote_id).unwrap();
                let parent_ref = node.parent_id.as_ref().and_then(|pid| {
                    id_to_ref.get(pid.as_str()).cloned()
                });
                NormalizedChange {
                    ref_name: ref_name.clone(),
                    deleted: false,
                    name: Some(node.name.clone()),
                    parent_ref,
                    node_type: Some(node.node_type),
                }
            }
        })
        .collect();

    // Final tree state (excluding root)
    let final_tree = remote.nodes.values()
        .filter(|n| !n.name.is_empty())
        .map(|n| {
            let ref_name = id_to_ref.get(n.id.as_str())
                .cloned()
                .unwrap_or_else(|| n.id.as_str().to_string());
            let parent_ref = n.parent_id.as_ref().and_then(|pid| {
                id_to_ref.get(pid.as_str()).cloned()
            });
            NormalizedNode {
                ref_name,
                name: n.name.clone(),
                parent_ref,
                node_type: n.node_type,
                md5sum: n.md5sum.clone(),
            }
        })
        .collect();

    ReplayResult { changes, final_tree }
}
```

**Step 2: Run `cargo check --test changes_feed_conformance`**

Expected: compiles

**Step 3: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: add MockRemote replayer for conformance tests"
```

---

### Task 3: Implement the `CozyClient` replayer

**Files:**
- Modify: `tests/changes_feed_conformance.rs`

**Step 1: Add the `TestCozy` fixture (reuse pattern from integration_tests.rs)**

Copy the `TestCozy` struct and `cozy_stack_available()` helper from `tests/integration_tests.rs`.

**Step 2: Add the async replayer for CozyClient**

This replays the same `RemoteAction` list against a real cozy-stack instance, mapping `ref_name` to the real `RemoteId` returned by the API. After replay, it fetches the changes feed and builds the same normalized structures.

```rust
async fn replay_on_cozy(actions: &[RemoteAction]) -> ReplayResult {
    let cozy = TestCozy::setup();
    let client = cozy.client();

    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let root_id = RemoteId::new("io.cozy.files.root-dir");

    // Get initial seq (skip default dirs created by instance setup)
    let initial = client.fetch_changes(None).await.unwrap();
    let since_seq = initial.last_seq.clone();

    for action in actions {
        match action {
            RemoteAction::CreateDir { ref_name, parent_ref, name } => {
                let parent_id = parent_ref.as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(&root_id);
                let node = client.create_directory(parent_id, name).await.unwrap();
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::CreateFile { ref_name, parent_ref, name, content } => {
                let parent_id = parent_ref.as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(&root_id);
                let md5 = compute_md5_from_bytes(content);
                let node = client.upload_file(parent_id, name, content.clone(), &md5)
                    .await.unwrap();
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::Rename { ref_name, new_name } => {
                let id = &ref_to_id[ref_name];
                // Fetch current parent to keep it unchanged
                let changes = client.fetch_changes(None).await.unwrap();
                let current = changes.results.iter()
                    .find(|r| r.node.id == *id && !r.deleted)
                    .unwrap();
                let parent_id = current.node.parent_id.clone().unwrap();
                client.move_node(id, &parent_id, new_name).await.unwrap();
            }
            RemoteAction::Move { ref_name, new_parent_ref } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = &ref_to_id[new_parent_ref];
                let changes = client.fetch_changes(None).await.unwrap();
                let current = changes.results.iter()
                    .find(|r| r.node.id == *id && !r.deleted)
                    .unwrap();
                let name = current.node.name.clone();
                client.move_node(id, new_parent_id, &name).await.unwrap();
            }
            RemoteAction::MoveAndRename { ref_name, new_parent_ref, new_name } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = &ref_to_id[new_parent_ref];
                client.move_node(id, new_parent_id, new_name).await.unwrap();
            }
            RemoteAction::Trash { ref_name } => {
                let id = &ref_to_id[ref_name];
                client.trash(id).await.unwrap();
            }
        }
    }

    // Fetch changes since our baseline
    let changes_resp = client.fetch_changes(Some(&since_seq)).await.unwrap();

    let changes = changes_resp.results.iter()
        .filter_map(|r| {
            let ref_name = id_to_ref.get(r.node.id.as_str())?.clone();
            if r.deleted {
                Some(NormalizedChange {
                    ref_name,
                    deleted: true,
                    name: None,
                    parent_ref: None,
                    node_type: None,
                })
            } else {
                let parent_ref = r.node.parent_id.as_ref().and_then(|pid| {
                    id_to_ref.get(pid.as_str()).cloned()
                });
                Some(NormalizedChange {
                    ref_name: ref_name.clone(),
                    deleted: false,
                    name: Some(r.node.name.clone()),
                    parent_ref,
                    node_type: Some(r.node.node_type),
                })
            }
        })
        .collect();

    // Build final tree from the latest changes (last state per ref_name)
    let mut latest_by_ref: HashMap<String, NormalizedNode> = HashMap::new();
    for r in &changes_resp.results {
        if let Some(ref_name) = id_to_ref.get(r.node.id.as_str()) {
            if r.deleted {
                latest_by_ref.remove(ref_name);
            } else {
                let parent_ref = r.node.parent_id.as_ref().and_then(|pid| {
                    id_to_ref.get(pid.as_str()).cloned()
                });
                latest_by_ref.insert(ref_name.clone(), NormalizedNode {
                    ref_name: ref_name.clone(),
                    name: r.node.name.clone(),
                    parent_ref,
                    node_type: r.node.node_type,
                    md5sum: r.node.md5sum.clone(),
                });
            }
        }
    }
    let final_tree = latest_by_ref.into_values().collect();

    ReplayResult { changes, final_tree }
}
```

**Step 2: Run `cargo check --test changes_feed_conformance`**

Expected: compiles

**Step 3: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: add CozyClient replayer for conformance tests"
```

---

### Task 4: Add the comparison logic and first scenario

**Files:**
- Modify: `tests/changes_feed_conformance.rs`

**Step 1: Add comparison function**

The comparison focuses on **structural equivalence** — same set of ref_names appear in changes, same final tree. We compare:
- Final tree: same nodes exist with same names, parents, types, and checksums (unordered)
- Changes feed: same ref_names appear, same deleted flags, same final name/parent for non-deleted entries

We do NOT compare ordering of changes (CouchDB doesn't guarantee ordering within a batch), nor do we compare the number of intermediate changes for the same node (CouchDB may coalesce).

```rust
use std::collections::BTreeSet;

fn compare_results(scenario_name: &str, mock: &ReplayResult, cozy: &ReplayResult) {
    // 1. Compare final tree state (unordered)
    let mock_tree: BTreeSet<String> = mock.final_tree.iter()
        .map(|n| format!("{}:{} parent={:?} type={:?} md5={:?}",
            n.ref_name, n.name, n.parent_ref, n.node_type, n.md5sum))
        .collect();
    let cozy_tree: BTreeSet<String> = cozy.final_tree.iter()
        .map(|n| format!("{}:{} parent={:?} type={:?} md5={:?}",
            n.ref_name, n.name, n.parent_ref, n.node_type, n.md5sum))
        .collect();

    if mock_tree != cozy_tree {
        let mock_only: BTreeSet<_> = mock_tree.difference(&cozy_tree).collect();
        let cozy_only: BTreeSet<_> = cozy_tree.difference(&mock_tree).collect();
        panic!(
            "[{scenario_name}] Final tree mismatch!\n  mock only: {mock_only:?}\n  cozy only: {cozy_only:?}"
        );
    }

    // 2. Compare changes: which ref_names appeared and their latest state
    // Coalesce changes per ref_name (keep last entry)
    let mock_latest = coalesce_changes(&mock.changes);
    let cozy_latest = coalesce_changes(&cozy.changes);

    if mock_latest != cozy_latest {
        panic!(
            "[{scenario_name}] Changes feed mismatch!\n  mock: {mock_latest:#?}\n  cozy: {cozy_latest:#?}"
        );
    }
}

fn coalesce_changes(changes: &[NormalizedChange]) -> BTreeSet<String> {
    let mut latest: HashMap<String, &NormalizedChange> = HashMap::new();
    for c in changes {
        latest.insert(c.ref_name.clone(), c);
    }
    latest.into_values()
        .map(|c| format!("{}:deleted={} name={:?} parent={:?}",
            c.ref_name, c.deleted, c.name, c.parent_ref))
        .collect()
}
```

**Step 2: Add the first scenario test**

```rust
fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "create_dir_then_file_inside_then_rename_dir",
            actions: vec![
                RemoteAction::CreateDir {
                    ref_name: "dir_a".into(),
                    parent_ref: None,
                    name: "photos".into(),
                },
                RemoteAction::CreateFile {
                    ref_name: "file_1".into(),
                    parent_ref: Some("dir_a".into()),
                    name: "sunset.jpg".into(),
                    content: b"image data".to_vec(),
                },
                RemoteAction::Rename {
                    ref_name: "dir_a".into(),
                    new_name: "vacation-photos".into(),
                },
            ],
        },
    ]
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn changes_feed_conformance() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    for scenario in scenarios() {
        eprintln!("Running scenario: {}", scenario.name);
        let mock_result = replay_on_mock(&scenario.actions);
        let cozy_result = replay_on_cozy(&scenario.actions).await;
        compare_results(scenario.name, &mock_result, &cozy_result);
        eprintln!("  ✓ {}", scenario.name);
    }
}
```

**Step 3: Run the test against cozy-stack**

```bash
cargo test --test changes_feed_conformance -- --ignored --nocapture
```

Expected: either PASS (mock matches cozy), or FAIL revealing a behavioral difference that needs fixing.

**Step 4: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: first conformance scenario - create dir, file inside, rename dir"
```

---

### Task 5: Add more scenarios

**Files:**
- Modify: `tests/changes_feed_conformance.rs`

**Step 1: Add scenarios for the cases you mentioned and more edge cases**

```rust
// Add these to the scenarios() function:

Scenario {
    name: "move_dir_with_nested_subdirs_and_files",
    actions: vec![
        RemoteAction::CreateDir {
            ref_name: "target".into(),
            parent_ref: None,
            name: "archive".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "dir_a".into(),
            parent_ref: None,
            name: "projects".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "dir_b".into(),
            parent_ref: Some("dir_a".into()),
            name: "2025".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_1".into(),
            parent_ref: Some("dir_b".into()),
            name: "report.pdf".into(),
            content: b"pdf content".to_vec(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_2".into(),
            parent_ref: Some("dir_a".into()),
            name: "readme.txt".into(),
            content: b"readme".to_vec(),
        },
        // Move the whole "projects" tree into "archive"
        RemoteAction::Move {
            ref_name: "dir_a".into(),
            new_parent_ref: "target".into(),
        },
    ],
},

Scenario {
    name: "trash_dir_with_children",
    actions: vec![
        RemoteAction::CreateDir {
            ref_name: "dir_a".into(),
            parent_ref: None,
            name: "old-stuff".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_1".into(),
            parent_ref: Some("dir_a".into()),
            name: "doc.txt".into(),
            content: b"old content".to_vec(),
        },
        RemoteAction::CreateDir {
            ref_name: "dir_b".into(),
            parent_ref: Some("dir_a".into()),
            name: "subdir".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_2".into(),
            parent_ref: Some("dir_b".into()),
            name: "nested.txt".into(),
            content: b"nested content".to_vec(),
        },
        RemoteAction::Trash {
            ref_name: "dir_a".into(),
        },
    ],
},

Scenario {
    name: "create_rename_move_combined",
    actions: vec![
        RemoteAction::CreateDir {
            ref_name: "src".into(),
            parent_ref: None,
            name: "source".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "dst".into(),
            parent_ref: None,
            name: "destination".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_1".into(),
            parent_ref: Some("src".into()),
            name: "data.csv".into(),
            content: b"a,b,c".to_vec(),
        },
        // Rename file, then move it to another dir
        RemoteAction::Rename {
            ref_name: "file_1".into(),
            new_name: "data-v2.csv".into(),
        },
        RemoteAction::Move {
            ref_name: "file_1".into(),
            new_parent_ref: "dst".into(),
        },
    ],
},

Scenario {
    name: "move_and_rename_dir_simultaneously",
    actions: vec![
        RemoteAction::CreateDir {
            ref_name: "parent_a".into(),
            parent_ref: None,
            name: "area-a".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "parent_b".into(),
            parent_ref: None,
            name: "area-b".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "moving_dir".into(),
            parent_ref: Some("parent_a".into()),
            name: "work".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_1".into(),
            parent_ref: Some("moving_dir".into()),
            name: "task.md".into(),
            content: b"# Tasks".to_vec(),
        },
        // Move + rename in a single API call
        RemoteAction::MoveAndRename {
            ref_name: "moving_dir".into(),
            new_parent_ref: "parent_b".into(),
            new_name: "completed-work".into(),
        },
    ],
},

Scenario {
    name: "deeply_nested_then_trash_middle",
    actions: vec![
        RemoteAction::CreateDir {
            ref_name: "a".into(),
            parent_ref: None,
            name: "a".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "b".into(),
            parent_ref: Some("a".into()),
            name: "b".into(),
        },
        RemoteAction::CreateDir {
            ref_name: "c".into(),
            parent_ref: Some("b".into()),
            name: "c".into(),
        },
        RemoteAction::CreateFile {
            ref_name: "file_deep".into(),
            parent_ref: Some("c".into()),
            name: "deep.txt".into(),
            content: b"deep".to_vec(),
        },
        // Trash the middle directory "b" — what happens to "c" and "file_deep"?
        RemoteAction::Trash {
            ref_name: "b".into(),
        },
    ],
},
```

**Step 2: Run the tests**

```bash
cargo test --test changes_feed_conformance -- --ignored --nocapture
```

**Step 3: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: add conformance scenarios for moves, trash, nesting"
```

---

### Task 6: Fix MockRemote discrepancies

Based on the test results from Task 5, some scenarios will likely fail. The most probable discrepancies:

1. **Trash behavior:** When cozy-stack trashes a directory, the changes feed likely shows the parent node as moved to trash (with `dir_id` = `io.cozy.files.trash-dir`), NOT as deleted. Children may or may not appear. The `MockRemote` uses `delete_node` which records deletions. Fix: use `move_node` to the trash dir instead, matching what `SimulationRunner::apply_remote_trash` already does.

2. **Children in move changes:** When a directory is moved, cozy-stack may or may NOT emit change records for children. If it doesn't, `MockRemote` is correct. If it does, we need to add child change records in `MockRemote.move_node`.

3. **Coalescing:** If a node is created then renamed, cozy-stack may report only one change entry (the latest state). `MockRemote` records both. This is fine if we coalesce during comparison, but we should also consider whether the `fetch_remote_changes` consumer in the simulator handles this correctly.

**Files:**
- Modify: `src/simulator/mock_remote.rs` — fix any behavioral differences found
- Modify: `tests/changes_feed_conformance.rs` — adjust comparison if needed

**Step 1: Analyze test failures**

Run tests, read failure output, identify which behaviors differ.

**Step 2: Fix MockRemote to match cozy-stack behavior**

Apply minimal fixes to align behavior. Each fix should be:
- Targeted (change one behavior at a time)
- Tested (re-run conformance test to verify it passes)

**Step 3: Re-run all tests**

```bash
cargo test -q
cargo test --test changes_feed_conformance -- --ignored --nocapture
```

Ensure no existing simulator tests break.

**Step 4: Commit**

```bash
git add src/simulator/mock_remote.rs tests/changes_feed_conformance.rs
git commit -m "fix: align MockRemote changes feed with cozy-stack behavior"
```

---

### Task 7: Add a "fetch changes incrementally" scenario

This tests the important case where changes are fetched in multiple batches (simulating what the sync client does — fetch, sync, fetch again).

**Files:**
- Modify: `tests/changes_feed_conformance.rs`

**Step 1: Add incremental fetch scenarios**

These test that fetching changes in two rounds (before and after a mutation) produces the same results in mock and cozy-stack.

```rust
/// A scenario with a checkpoint: actions before sync, then actions after.
struct IncrementalScenario {
    name: &'static str,
    phase1_actions: Vec<RemoteAction>,
    phase2_actions: Vec<RemoteAction>,
}
```

Add test function that:
1. Replays `phase1_actions`, fetches changes (records as `changes_1`)
2. Replays `phase2_actions`, fetches changes since `phase1` seq (records as `changes_2`)
3. Compares both `changes_1` and `changes_2` between mock and cozy

Scenarios:
- Create dir + file, fetch, rename dir, fetch → verify second fetch only shows the rename
- Create dir tree, fetch, trash parent, fetch → verify second fetch shows correct trash representation
- Create file, fetch, modify file content, fetch → verify second fetch shows content update

**Step 2: Run tests**

```bash
cargo test --test changes_feed_conformance -- --ignored --nocapture
```

**Step 3: Commit**

```bash
git add tests/changes_feed_conformance.rs
git commit -m "test: add incremental changes feed conformance scenarios"
```

---

### Task 8: Lint, format, and final verification

**Step 1: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features
```

Fix any warnings.

**Step 2: Run all tests**

```bash
cargo test -q
cargo test --test changes_feed_conformance -- --ignored --nocapture
```

**Step 3: Commit**

```bash
git add -u
git commit -m "chore: lint and format conformance tests"
```

---

## Summary of Behavioral Properties Tested

| Property | Scenario |
|---|---|
| Dir create → file inside → rename dir | `create_dir_then_file_inside_then_rename_dir` |
| Move dir with nested subdirs and files | `move_dir_with_nested_subdirs_and_files` |
| Trash dir cascades to children | `trash_dir_with_children` |
| Rename then move a file | `create_rename_move_combined` |
| Simultaneous move + rename | `move_and_rename_dir_simultaneously` |
| Trash middle of deep nesting | `deeply_nested_then_trash_middle` |
| Incremental fetch sees only new changes | `IncrementalScenario` tests |

## Potential MockRemote Fixes (Hypotheses)

These are educated guesses about what will need fixing. The conformance tests will confirm:

1. `MockRemote::delete_node` for trash is wrong — trash should use `move_node` to the trash dir (the `SimulationRunner` already does this correctly, but `replay_on_mock` in the conformance test should match)
2. The changes feed's `get_changes_since` for a node that was modified multiple times may need to only return the latest state (like CouchDB does) instead of multiple entries
3. Children of a moved directory may or may not need change records — the tests will tell us
