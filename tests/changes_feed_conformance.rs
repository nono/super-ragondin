//! Conformance tests: verify `MockRemote` changes feed matches real cozy-stack.
//!
//! Run with:
//! ```bash
//! cargo test --test changes_feed_conformance -- --ignored
//! ```

use cozy_desktop::model::{NodeType, RemoteId, RemoteNode};
use cozy_desktop::remote::client::CozyClient;
use cozy_desktop::simulator::mock_remote::MockRemote;
use cozy_desktop::util::compute_md5_from_bytes;
use std::collections::{BTreeSet, HashMap};
use std::process::Command;
use tempfile::TempDir;

/// A remote mutation that can be replayed against both `MockRemote` and `CozyClient`.
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

// ==================== Normalized types for comparison ====================

/// Normalized representation of a single change entry for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedChange {
    ref_name: String,
    deleted: bool,
    name: Option<String>,
    parent_ref: Option<String>,
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
    changes: Vec<NormalizedChange>,
    final_tree: Vec<NormalizedNode>,
}

// ==================== MockRemote replayer ====================

#[allow(clippy::too_many_lines)]
fn replay_on_mock(actions: &[RemoteAction]) -> ReplayResult {
    let mut remote = MockRemote::new();
    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let mut id_counter = 0u32;

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
            RemoteAction::CreateDir {
                ref_name,
                parent_ref,
                name,
            } => {
                id_counter += 1;
                let id = RemoteId::new(format!("mock-{id_counter}"));
                let parent_id = parent_ref
                    .as_ref()
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
            RemoteAction::CreateFile {
                ref_name,
                parent_ref,
                name,
                content,
            } => {
                id_counter += 1;
                let id = RemoteId::new(format!("mock-{id_counter}"));
                let parent_id = parent_ref
                    .as_ref()
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
            RemoteAction::Move {
                ref_name,
                new_parent_ref,
            } => {
                let id = &ref_to_id[ref_name];
                let name = remote.get_node(id).unwrap().name.clone();
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, name);
            }
            RemoteAction::MoveAndRename {
                ref_name,
                new_parent_ref,
                new_name,
            } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, new_name.clone());
            }
            RemoteAction::Trash { ref_name } => {
                let id = &ref_to_id[ref_name];
                remote.trash_node(id);
            }
        }
    }

    let trash_id = RemoteId::new("io.cozy.files.trash-dir");
    let changes = remote
        .get_all_changes_since(seq_after_root)
        .into_iter()
        .filter(|c| c.remote_id != trash_id)
        .filter_map(|c| {
            let ref_name = id_to_ref
                .get(c.remote_id.as_str())
                .cloned()
                .unwrap_or_else(|| c.remote_id.as_str().to_string());
            if c.deleted {
                Some(NormalizedChange {
                    ref_name,
                    deleted: true,
                    name: None,
                    parent_ref: None,
                    node_type: None,
                })
            } else {
                let node = remote.get_node(&c.remote_id)?;
                let parent_ref = node
                    .parent_id
                    .as_ref()
                    .and_then(|pid| id_to_ref.get(pid.as_str()).cloned());
                Some(NormalizedChange {
                    ref_name,
                    deleted: false,
                    name: Some(node.name.clone()),
                    parent_ref,
                    node_type: Some(node.node_type),
                })
            }
        })
        .collect();

    let final_tree = remote
        .nodes
        .values()
        .filter(|n| {
            !n.name.is_empty() && n.id != trash_id && !is_under_trash_mock(&remote, n, &trash_id)
        })
        .map(|n| {
            let ref_name = id_to_ref
                .get(n.id.as_str())
                .cloned()
                .unwrap_or_else(|| n.id.as_str().to_string());
            let parent_ref = n
                .parent_id
                .as_ref()
                .and_then(|pid| id_to_ref.get(pid.as_str()).cloned());
            NormalizedNode {
                ref_name,
                name: n.name.clone(),
                parent_ref,
                node_type: n.node_type,
                md5sum: n.md5sum.clone(),
            }
        })
        .collect();

    ReplayResult {
        changes,
        final_tree,
    }
}

fn is_under_trash_mock(remote: &MockRemote, node: &RemoteNode, trash_id: &RemoteId) -> bool {
    let mut current = node.parent_id.clone();
    let mut visited = std::collections::HashSet::new();
    while let Some(ref pid) = current {
        if pid == trash_id {
            return true;
        }
        if !visited.insert(pid.clone()) {
            return false;
        }
        if let Some(parent) = remote.get_node(pid) {
            current.clone_from(&parent.parent_id);
        } else {
            return false;
        }
    }
    false
}

// ==================== CozyClient replayer ====================

struct TestCozy {
    domain: String,
    access_token: String,
    #[allow(dead_code)]
    sync_dir: TempDir,
}

impl TestCozy {
    fn setup() -> Self {
        let id = &uuid::Uuid::new_v4().to_string()[..8];
        let domain = format!("test-{id}.localhost:8080");

        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "add",
                &domain,
                "--passphrase",
                "cozy",
                "--apps",
                "home,drive",
                "--email",
                "test@cozy.localhost",
                "--public-name",
                "Test",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to create instance: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "client-oauth",
                &domain,
                "http://localhost/",
                "conformance-test",
                "github.com/nono/cozy-desktop-ng",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to create OAuth client: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let client_id = String::from_utf8(output.stdout)
            .expect("Invalid UTF-8 in client_id")
            .trim()
            .to_string();

        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "token-oauth",
                &domain,
                &client_id,
                "io.cozy.files",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to get token: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let access_token = String::from_utf8(output.stdout)
            .expect("Invalid UTF-8 in token")
            .trim()
            .to_string();

        Self {
            domain,
            access_token,
            sync_dir: TempDir::new().expect("Failed to create sync dir"),
        }
    }

    fn client(&self) -> CozyClient {
        CozyClient::new(&format!("http://{}", self.domain), &self.access_token)
    }
}

impl Drop for TestCozy {
    fn drop(&mut self) {
        match Command::new("cozy-stack")
            .args(["instances", "rm", "--force", &self.domain])
            .output()
        {
            Ok(output) if !output.status.success() => {
                eprintln!(
                    "Warning: failed to clean up instance `{}`:\n{}",
                    self.domain,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to clean up instance `{}`: {}",
                    self.domain, e
                );
            }
            _ => {}
        }
    }
}

fn cozy_stack_available() -> bool {
    Command::new("cozy-stack")
        .args(["instances", "ls"])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[allow(clippy::too_many_lines)]
async fn replay_on_cozy(
    actions: &[RemoteAction],
) -> Result<ReplayResult, Box<dyn std::error::Error>> {
    let cozy = TestCozy::setup();
    let client = cozy.client();

    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let mut ref_to_node: HashMap<String, RemoteNode> = HashMap::new();
    let root_id = RemoteId::new("io.cozy.files.root-dir");

    // Get initial seq (skip default dirs created by instance setup)
    let initial = client.fetch_changes(None).await?;
    let since_seq = initial.last_seq.clone();

    for action in actions {
        match action {
            RemoteAction::CreateDir {
                ref_name,
                parent_ref,
                name,
            } => {
                let parent_id = parent_ref
                    .as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(&root_id);
                let node = client.create_directory(parent_id, name).await?;
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
                ref_to_node.insert(ref_name.clone(), node);
            }
            RemoteAction::CreateFile {
                ref_name,
                parent_ref,
                name,
                content,
            } => {
                let parent_id = parent_ref
                    .as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(&root_id);
                let md5 = compute_md5_from_bytes(content);
                let node = client
                    .upload_file(parent_id, name, content.clone(), &md5)
                    .await?;
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
                ref_to_node.insert(ref_name.clone(), node);
            }
            RemoteAction::Rename { ref_name, new_name } => {
                let id = &ref_to_id[ref_name];
                let current_node = &ref_to_node[ref_name];
                let parent_id = current_node
                    .parent_id
                    .as_ref()
                    .ok_or_else(|| format!("Node '{ref_name}' has no parent, cannot rename"))?;
                let updated_node = client.move_node(id, parent_id, new_name).await?;
                ref_to_node.insert(ref_name.clone(), updated_node);
            }
            RemoteAction::Move {
                ref_name,
                new_parent_ref,
            } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = &ref_to_id[new_parent_ref];
                let current_node = &ref_to_node[ref_name];
                let name = &current_node.name;
                let updated_node = client.move_node(id, new_parent_id, name).await?;
                ref_to_node.insert(ref_name.clone(), updated_node);
            }
            RemoteAction::MoveAndRename {
                ref_name,
                new_parent_ref,
                new_name,
            } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = &ref_to_id[new_parent_ref];
                let updated_node = client.move_node(id, new_parent_id, new_name).await?;
                ref_to_node.insert(ref_name.clone(), updated_node);
            }
            RemoteAction::Trash { ref_name } => {
                let id = &ref_to_id[ref_name];
                client.trash(id).await?;
                ref_to_node.remove(ref_name);
            }
        }
    }

    // Fetch changes since our baseline
    let changes_resp = client.fetch_changes(Some(&since_seq)).await?;
    Ok(collect_cozy_result(&changes_resp.results, &id_to_ref))
}

// ==================== Comparison ====================

fn compare_results(scenario_name: &str, mock: &ReplayResult, cozy: &ReplayResult) {
    // 1. Compare final tree state (unordered)
    let mock_tree: BTreeSet<String> = mock
        .final_tree
        .iter()
        .map(|n| {
            format!(
                "{}:{} parent={:?} type={:?} md5={:?}",
                n.ref_name, n.name, n.parent_ref, n.node_type, n.md5sum
            )
        })
        .collect();
    let cozy_tree: BTreeSet<String> = cozy
        .final_tree
        .iter()
        .map(|n| {
            format!(
                "{}:{} parent={:?} type={:?} md5={:?}",
                n.ref_name, n.name, n.parent_ref, n.node_type, n.md5sum
            )
        })
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
        let mock_only: BTreeSet<_> = mock_latest.difference(&cozy_latest).collect();
        let cozy_only: BTreeSet<_> = cozy_latest.difference(&mock_latest).collect();
        panic!(
            "[{scenario_name}] Changes feed mismatch!\n  mock only: {mock_only:?}\n  cozy only: {cozy_only:?}"
        );
    }
}

fn coalesce_changes(changes: &[NormalizedChange]) -> BTreeSet<String> {
    let mut latest: HashMap<String, &NormalizedChange> = HashMap::new();
    for c in changes {
        latest.insert(c.ref_name.clone(), c);
    }
    latest
        .into_values()
        .map(|c| {
            format!(
                "{}:deleted={} name={:?} parent={:?}",
                c.ref_name, c.deleted, c.name, c.parent_ref
            )
        })
        .collect()
}

// ==================== Scenarios ====================

#[allow(clippy::too_many_lines)]
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
                RemoteAction::Trash {
                    ref_name: "b".into(),
                },
            ],
        },
    ]
}

// ==================== Test ====================

// ==================== Incremental scenarios ====================

/// A scenario with a checkpoint: actions in two phases with a fetch in between.
struct IncrementalScenario {
    name: &'static str,
    phase1_actions: Vec<RemoteAction>,
    phase2_actions: Vec<RemoteAction>,
}

fn incremental_scenarios() -> Vec<IncrementalScenario> {
    vec![
        IncrementalScenario {
            name: "incremental_create_then_rename",
            phase1_actions: vec![
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
            ],
            phase2_actions: vec![RemoteAction::Rename {
                ref_name: "dir_a".into(),
                new_name: "vacation-photos".into(),
            }],
        },
        IncrementalScenario {
            name: "incremental_create_tree_then_trash",
            phase1_actions: vec![
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
            ],
            phase2_actions: vec![RemoteAction::Trash {
                ref_name: "dir_a".into(),
            }],
        },
        IncrementalScenario {
            name: "incremental_create_then_move_to_new_dir",
            phase1_actions: vec![
                RemoteAction::CreateDir {
                    ref_name: "src".into(),
                    parent_ref: None,
                    name: "source".into(),
                },
                RemoteAction::CreateFile {
                    ref_name: "file_1".into(),
                    parent_ref: Some("src".into()),
                    name: "data.csv".into(),
                    content: b"a,b,c".to_vec(),
                },
            ],
            phase2_actions: vec![
                RemoteAction::CreateDir {
                    ref_name: "dst".into(),
                    parent_ref: None,
                    name: "destination".into(),
                },
                RemoteAction::Move {
                    ref_name: "file_1".into(),
                    new_parent_ref: "dst".into(),
                },
            ],
        },
    ]
}

/// Replay an incremental scenario on the mock in two phases.
fn replay_incremental_on_mock(
    phase1: &[RemoteAction],
    phase2: &[RemoteAction],
) -> (ReplayResult, ReplayResult) {
    let mut remote = MockRemote::new();
    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let mut id_counter = 0u32;

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

    // Phase 1
    replay_actions_on_mock(
        &mut remote,
        &root_id,
        &mut ref_to_id,
        &mut id_to_ref,
        &mut id_counter,
        phase1,
    );

    let phase1_result = collect_mock_result(&remote, &id_to_ref, &ref_to_id, seq_after_root);
    let seq_after_phase1 = remote.current_seq();

    // Phase 2
    replay_actions_on_mock(
        &mut remote,
        &root_id,
        &mut ref_to_id,
        &mut id_to_ref,
        &mut id_counter,
        phase2,
    );

    let phase2_result = collect_mock_result(&remote, &id_to_ref, &ref_to_id, seq_after_phase1);

    (phase1_result, phase2_result)
}

fn replay_actions_on_mock(
    remote: &mut MockRemote,
    root_id: &RemoteId,
    ref_to_id: &mut HashMap<String, RemoteId>,
    id_to_ref: &mut HashMap<String, String>,
    id_counter: &mut u32,
    actions: &[RemoteAction],
) {
    for action in actions {
        match action {
            RemoteAction::CreateDir {
                ref_name,
                parent_ref,
                name,
            } => {
                *id_counter += 1;
                let id = RemoteId::new(format!("mock-{}", *id_counter));
                let parent_id = parent_ref
                    .as_ref()
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
            RemoteAction::CreateFile {
                ref_name,
                parent_ref,
                name,
                content,
            } => {
                *id_counter += 1;
                let id = RemoteId::new(format!("mock-{}", *id_counter));
                let parent_id = parent_ref
                    .as_ref()
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
            RemoteAction::Move {
                ref_name,
                new_parent_ref,
            } => {
                let id = &ref_to_id[ref_name];
                let name = remote.get_node(id).unwrap().name.clone();
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, name);
            }
            RemoteAction::MoveAndRename {
                ref_name,
                new_parent_ref,
                new_name,
            } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = ref_to_id[new_parent_ref].clone();
                remote.move_node(id, new_parent_id, new_name.clone());
            }
            RemoteAction::Trash { ref_name } => {
                let id = &ref_to_id[ref_name];
                remote.trash_node(id);
            }
        }
    }
}

fn collect_mock_result(
    remote: &MockRemote,
    id_to_ref: &HashMap<String, String>,
    ref_to_id: &HashMap<String, RemoteId>,
    since_seq: u64,
) -> ReplayResult {
    let trash_id = RemoteId::new("io.cozy.files.trash-dir");
    let changes = remote
        .get_all_changes_since(since_seq)
        .into_iter()
        .filter(|c| c.remote_id != trash_id)
        .filter_map(|c| {
            let ref_name = id_to_ref
                .get(c.remote_id.as_str())
                .cloned()
                .unwrap_or_else(|| c.remote_id.as_str().to_string());
            if c.deleted {
                Some(NormalizedChange {
                    ref_name,
                    deleted: true,
                    name: None,
                    parent_ref: None,
                    node_type: None,
                })
            } else {
                let node = remote.get_node(&c.remote_id)?;
                let parent_ref = node
                    .parent_id
                    .as_ref()
                    .and_then(|pid| id_to_ref.get(pid.as_str()).cloned());
                Some(NormalizedChange {
                    ref_name,
                    deleted: false,
                    name: Some(node.name.clone()),
                    parent_ref,
                    node_type: Some(node.node_type),
                })
            }
        })
        .collect::<Vec<_>>();

    // Build final_tree from changes (last state per ref_name),
    // consistent with how the cozy replayer does it.
    let trash_id = RemoteId::new("io.cozy.files.trash-dir");
    let mut latest_by_ref: HashMap<String, NormalizedNode> = HashMap::new();
    for c in &changes {
        let is_trashed = ref_to_id
            .get(&c.ref_name)
            .and_then(|id| remote.get_node(id))
            .is_some_and(|n| n.id == trash_id || is_under_trash_mock(remote, n, &trash_id));
        if c.deleted || is_trashed {
            latest_by_ref.remove(&c.ref_name);
        } else {
            let md5sum = ref_to_id
                .get(&c.ref_name)
                .and_then(|id| remote.get_node(id))
                .and_then(|n| n.md5sum.clone());
            latest_by_ref.insert(
                c.ref_name.clone(),
                NormalizedNode {
                    ref_name: c.ref_name.clone(),
                    name: c.name.clone().unwrap_or_default(),
                    parent_ref: c.parent_ref.clone(),
                    node_type: c.node_type.unwrap_or(NodeType::File),
                    md5sum,
                },
            );
        }
    }
    let final_tree = latest_by_ref.into_values().collect();

    ReplayResult {
        changes,
        final_tree,
    }
}

/// Replay an incremental scenario on the real cozy-stack in two phases.
#[allow(clippy::too_many_lines)]
async fn replay_incremental_on_cozy(
    phase1: &[RemoteAction],
    phase2: &[RemoteAction],
) -> (ReplayResult, ReplayResult) {
    let cozy = TestCozy::setup();
    let client = cozy.client();

    let mut ref_to_id: HashMap<String, RemoteId> = HashMap::new();
    let mut id_to_ref: HashMap<String, String> = HashMap::new();
    let root_id = RemoteId::new("io.cozy.files.root-dir");

    let initial = client.fetch_changes(None).await.unwrap();
    let since_seq = initial.last_seq.clone();

    // Phase 1
    replay_actions_on_cozy(&client, &root_id, &mut ref_to_id, &mut id_to_ref, phase1).await;

    let phase1_resp = client.fetch_changes(Some(&since_seq)).await.unwrap();
    let phase1_result = collect_cozy_result(&phase1_resp.results, &id_to_ref);
    let mid_seq = phase1_resp.last_seq.clone();

    // Phase 2
    replay_actions_on_cozy(&client, &root_id, &mut ref_to_id, &mut id_to_ref, phase2).await;

    let phase2_resp = client.fetch_changes(Some(&mid_seq)).await.unwrap();
    let phase2_result = collect_cozy_result(&phase2_resp.results, &id_to_ref);

    (phase1_result, phase2_result)
}

async fn replay_actions_on_cozy(
    client: &CozyClient,
    root_id: &RemoteId,
    ref_to_id: &mut HashMap<String, RemoteId>,
    id_to_ref: &mut HashMap<String, String>,
    actions: &[RemoteAction],
) {
    for action in actions {
        match action {
            RemoteAction::CreateDir {
                ref_name,
                parent_ref,
                name,
            } => {
                let parent_id = parent_ref
                    .as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(root_id);
                let node = client.create_directory(parent_id, name).await.unwrap();
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::CreateFile {
                ref_name,
                parent_ref,
                name,
                content,
            } => {
                let parent_id = parent_ref
                    .as_ref()
                    .and_then(|r| ref_to_id.get(r))
                    .unwrap_or(root_id);
                let md5 = compute_md5_from_bytes(content);
                let node = client
                    .upload_file(parent_id, name, content.clone(), &md5)
                    .await
                    .unwrap();
                ref_to_id.insert(ref_name.clone(), node.id.clone());
                id_to_ref.insert(node.id.as_str().to_string(), ref_name.clone());
            }
            RemoteAction::Rename { ref_name, new_name } => {
                let id = &ref_to_id[ref_name];
                let changes = client.fetch_changes(None).await.unwrap();
                let current = changes
                    .results
                    .iter()
                    .rev()
                    .find(|r| r.node.id == *id && !r.deleted)
                    .expect("node not found in changes for Rename");
                let parent_id = current.node.parent_id.clone().unwrap();
                client.move_node(id, &parent_id, new_name).await.unwrap();
            }
            RemoteAction::Move {
                ref_name,
                new_parent_ref,
            } => {
                let id = &ref_to_id[ref_name];
                let new_parent_id = &ref_to_id[new_parent_ref];
                let changes = client.fetch_changes(None).await.unwrap();
                let current = changes
                    .results
                    .iter()
                    .rev()
                    .find(|r| r.node.id == *id && !r.deleted)
                    .expect("node not found in changes for Move");
                let name = current.node.name.clone();
                client.move_node(id, new_parent_id, &name).await.unwrap();
            }
            RemoteAction::MoveAndRename {
                ref_name,
                new_parent_ref,
                new_name,
            } => {
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
}

fn collect_cozy_result(
    results: &[cozy_desktop::remote::client::ChangeResult],
    id_to_ref: &HashMap<String, String>,
) -> ReplayResult {
    let changes = results
        .iter()
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
                let parent_ref = r
                    .node
                    .parent_id
                    .as_ref()
                    .and_then(|pid| id_to_ref.get(pid.as_str()).cloned());
                Some(NormalizedChange {
                    ref_name,
                    deleted: false,
                    name: Some(r.node.name.clone()),
                    parent_ref,
                    node_type: Some(r.node.node_type),
                })
            }
        })
        .collect();

    // Build parent map from latest state of each node (for trash detection)
    let trash_id_str = "io.cozy.files.trash-dir";
    let mut parent_map: HashMap<String, Option<String>> = HashMap::new();
    for r in results {
        if !r.deleted {
            parent_map.insert(
                r.node.id.as_str().to_string(),
                r.node.parent_id.as_ref().map(|p| p.as_str().to_string()),
            );
        }
    }

    let is_in_trash = |node_id: &str| -> bool {
        let mut current = parent_map.get(node_id).and_then(|p| p.clone());
        let mut visited = std::collections::HashSet::new();
        while let Some(pid) = current {
            if pid == trash_id_str {
                return true;
            }
            if !visited.insert(pid.clone()) {
                return false;
            }
            current = parent_map.get(&pid).and_then(|p| p.clone());
        }
        false
    };

    let mut latest_by_ref: HashMap<String, NormalizedNode> = HashMap::new();
    for r in results {
        if let Some(ref_name) = id_to_ref.get(r.node.id.as_str()) {
            if r.deleted || is_in_trash(r.node.id.as_str()) {
                latest_by_ref.remove(ref_name);
            } else {
                let parent_ref = r
                    .node
                    .parent_id
                    .as_ref()
                    .and_then(|pid| id_to_ref.get(pid.as_str()).cloned());
                latest_by_ref.insert(
                    ref_name.clone(),
                    NormalizedNode {
                        ref_name: ref_name.clone(),
                        name: r.node.name.clone(),
                        parent_ref,
                        node_type: r.node.node_type,
                        md5sum: r.node.md5sum.clone(),
                    },
                );
            }
        }
    }
    let final_tree = latest_by_ref.into_values().collect();

    ReplayResult {
        changes,
        final_tree,
    }
}

// ==================== Tests ====================

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn changes_feed_conformance() -> Result<(), Box<dyn std::error::Error>> {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return Ok(());
    }

    for scenario in scenarios() {
        eprintln!("Running scenario: {}", scenario.name);
        let mock_result = replay_on_mock(&scenario.actions);
        let cozy_result = replay_on_cozy(&scenario.actions).await?;
        compare_results(scenario.name, &mock_result, &cozy_result);
        eprintln!("  ✓ {}", scenario.name);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn changes_feed_incremental_conformance() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    for scenario in incremental_scenarios() {
        eprintln!("Running incremental scenario: {}", scenario.name);
        let (mock_p1, mock_p2) =
            replay_incremental_on_mock(&scenario.phase1_actions, &scenario.phase2_actions);
        let (cozy_p1, cozy_p2) =
            replay_incremental_on_cozy(&scenario.phase1_actions, &scenario.phase2_actions).await;

        let phase1_name = format!("{} (phase 1)", scenario.name);
        compare_results(&phase1_name, &mock_p1, &cozy_p1);

        let phase2_name = format!("{} (phase 2)", scenario.name);
        compare_results(&phase2_name, &mock_p2, &cozy_p2);

        eprintln!("  ✓ {}", scenario.name);
    }
}
