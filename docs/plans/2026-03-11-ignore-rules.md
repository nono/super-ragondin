# Ignore Rules (syncignore) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add gitignore-compatible ignore rules so that certain files and folders (hidden files, editor temp files, OS metadata, etc.) are excluded from synchronization.

**Architecture:** A new `IgnoreRules` struct in `crates/sync/src/ignore.rs` loads patterns from a default embedded rules file and an optional user-customizable file. It exposes an `is_ignored(rel_path, is_dir) -> bool` method. The rules are checked at three points: (1) the local scanner filters out ignored entries during filesystem walk, (2) the planner skips ignored remote nodes, and (3) the watcher filters out events for ignored paths.

**Tech Stack:** The `ignore` crate by BurntSushi (from the ripgrep project) provides `gitignore::GitignoreBuilder` which handles all gitignore semantics: negation (`!`), basename matching, folder-only (`/` suffix), `**` globstar, and comment/blank lines. It's battle-tested (79M downloads) and avoids reimplementing the spec.

---

### Task 1: Add the `ignore` crate dependency

**Files:**
- Modify: `crates/sync/Cargo.toml`

**Step 1: Add the dependency**

```bash
cd crates/sync && cargo add ignore
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add crates/sync/Cargo.toml ../../Cargo.lock
git commit -m "build: add ignore crate dependency"
```

---

### Task 2: Create the default syncignore rules file

**Files:**
- Create: `crates/sync/src/config/syncignore`

**Step 1: Create the default rules file**

Create `crates/sync/src/config/syncignore` with the following content (adapted from the old cozy-desktop client):

```
# Default ignore rules for Super Ragondin
# See https://git-scm.com/docs/gitignore/#_pattern_format

# Hidden files and directories
.*

# Editor temp files
*.tmp
*.bak

# Emacs
*~
\#*\#

# LibreOffice
*.osl-tmp

# OpenOffice
*.lck

# Microsoft Office
~$*

# Vim
*.sw[px]

# Google Chrome partial downloads
*.crdownload
```

**Step 2: Commit**

```bash
git add crates/sync/src/config/syncignore
git commit -m "feat: add default syncignore rules file"
```

---

### Task 3: Implement `IgnoreRules` struct with tests (TDD)

**Files:**
- Create: `crates/sync/src/ignore.rs`
- Modify: `crates/sync/src/lib.rs` (add `pub mod ignore;`)

**Step 1: Write the failing tests**

Create `crates/sync/src/ignore.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_ignore_hidden_files() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored(".hidden", false));
        assert!(rules.is_ignored(".git", true));
    }

    #[test]
    fn default_rules_ignore_editor_temp_files() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored("file.tmp", false));
        assert!(rules.is_ignored("file.bak", false));
        assert!(rules.is_ignored("file~", false));
        assert!(rules.is_ignored("file.swp", false));
        assert!(rules.is_ignored("file.swx", false));
        assert!(rules.is_ignored("~$document.docx", false));
    }

    #[test]
    fn default_rules_allow_normal_files() {
        let rules = IgnoreRules::default_only();
        assert!(!rules.is_ignored("document.pdf", false));
        assert!(!rules.is_ignored("photo.jpg", false));
        assert!(!rules.is_ignored("notes.txt", false));
        assert!(!rules.is_ignored("project", true));
    }

    #[test]
    fn nested_paths_are_checked() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored("subdir/.hidden", false));
        assert!(rules.is_ignored("a/b/file.tmp", false));
        assert!(!rules.is_ignored("subdir/normal.txt", false));
    }

    #[test]
    fn user_rules_override_defaults() {
        let user_content = "*.log\n!important.log\n";
        let rules = IgnoreRules::with_user_rules(user_content);
        assert!(rules.is_ignored("debug.log", false));
        assert!(!rules.is_ignored("important.log", false));
        assert!(!rules.is_ignored("data.csv", false));
    }

    #[test]
    fn folder_only_patterns() {
        let user_content = "build/\n";
        let rules = IgnoreRules::with_user_rules(user_content);
        assert!(rules.is_ignored("build", true));
        assert!(!rules.is_ignored("build", false));
    }

    #[test]
    fn empty_user_rules_only_uses_defaults() {
        let rules = IgnoreRules::with_user_rules("");
        assert!(rules.is_ignored(".hidden", false));
        assert!(!rules.is_ignored("normal.txt", false));
    }
}
```

**Step 2: Add the module declaration**

In `crates/sync/src/lib.rs`, add:

```rust
pub mod ignore;
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-sync ignore`
Expected: compilation errors (structs/methods not defined yet)

**Step 4: Implement `IgnoreRules`**

Add the implementation above the tests in `crates/sync/src/ignore.rs`:

```rust
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

const DEFAULT_RULES: &str = include_str!("config/syncignore");

pub struct IgnoreRules {
    gitignore: Gitignore,
}

impl IgnoreRules {
    /// Create rules from the embedded default syncignore file only.
    #[must_use]
    pub fn default_only() -> Self {
        Self::build(DEFAULT_RULES, "")
    }

    /// Create rules from embedded defaults plus user-provided rules.
    ///
    /// User rules are appended after defaults, so they can override
    /// with negation patterns (`!pattern`).
    #[must_use]
    pub fn with_user_rules(user_content: &str) -> Self {
        Self::build(DEFAULT_RULES, user_content)
    }

    /// Load rules from the default file and an optional user rules file.
    ///
    /// If the user file does not exist or cannot be read, only defaults apply.
    #[must_use]
    pub fn load(user_rules_path: Option<&Path>) -> Self {
        let user_content = user_rules_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self::build(DEFAULT_RULES, &user_content)
    }

    fn build(default_content: &str, user_content: &str) -> Self {
        let mut builder = GitignoreBuilder::new("");
        for line in default_content.lines() {
            builder.add_line(None, line).ok();
        }
        for line in user_content.lines() {
            builder.add_line(None, line).ok();
        }
        let gitignore = builder.build().expect("failed to build gitignore rules");
        Self { gitignore }
    }

    /// Returns `true` if the given relative path should be ignored.
    ///
    /// `rel_path` is relative to the sync root (e.g. `"subdir/file.txt"`).
    /// `is_dir` indicates whether the path is a directory.
    #[must_use]
    pub fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        self.gitignore
            .matched_path_or_any_parents(rel_path, is_dir)
            .is_ignore()
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p super-ragondin-sync ignore`
Expected: all tests pass

**Step 6: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 7: Commit**

```bash
git add crates/sync/src/ignore.rs crates/sync/src/lib.rs
git commit -m "feat: implement IgnoreRules with default syncignore patterns"
```

---

### Task 4: Integrate ignore rules into the local scanner

**Files:**
- Modify: `crates/sync/src/local/scanner.rs`
- Modify: `crates/sync/tests/local_tests.rs`

**Step 1: Write the failing test**

Add to `crates/sync/tests/local_tests.rs`:

```rust
use super_ragondin_sync::ignore::IgnoreRules;

#[test]
fn test_scan_ignores_hidden_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("visible.txt"), b"hello").unwrap();
    fs::write(root.join(".hidden"), b"secret").unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), b"data").unwrap();
    fs::write(root.join("file.swp"), b"vim temp").unwrap();

    let rules = IgnoreRules::default_only();
    let scanner = Scanner::new(root);
    let nodes = scanner.scan_with_ignore(&rules).unwrap();

    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "visible.txt");
}

#[test]
fn test_scan_ignores_nested_hidden_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::create_dir(root.join("subdir")).unwrap();
    fs::write(root.join("subdir/normal.txt"), b"ok").unwrap();
    fs::write(root.join("subdir/.hidden"), b"secret").unwrap();

    let rules = IgnoreRules::default_only();
    let scanner = Scanner::new(root);
    let nodes = scanner.scan_with_ignore(&rules).unwrap();

    // subdir + normal.txt (but not .hidden)
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|n| n.name != ".hidden"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-sync test_scan_ignores`
Expected: compilation error (method `scan_with_ignore` not found)

**Step 3: Implement `scan_with_ignore`**

In `crates/sync/src/local/scanner.rs`:

1. Add import: `use crate::ignore::IgnoreRules;`
2. Add a `scan_with_ignore` method that delegates to a new `scan_recursive_with_ignore` method.
3. In the recursive method, compute the relative path of each entry and call `rules.is_ignored(rel_path, is_dir)`. If ignored, skip the entry (and don't recurse into ignored directories).

```rust
/// Scan all files and directories, filtering out ignored paths.
///
/// # Errors
/// Returns an error if filesystem operations fail.
pub fn scan_with_ignore(&self, rules: &IgnoreRules) -> Result<Vec<LocalNode>> {
    tracing::info!(root = %self.root.display(), "🔍 Starting local filesystem scan (with ignore rules)");
    let mut nodes = Vec::new();
    let mut inode_to_id: HashMap<(u64, u64), LocalFileId> = HashMap::new();

    let root_meta = fs::symlink_metadata(&self.root)?;
    let root_id = LocalFileId::new(root_meta.dev(), root_meta.ino());

    Self::scan_recursive_with_ignore(
        &self.root,
        &self.root,
        Some(&root_id),
        &mut nodes,
        &mut inode_to_id,
        rules,
    )?;
    tracing::info!(root = %self.root.display(), count = nodes.len(), "🔍 Scan complete");
    Ok(nodes)
}

fn scan_recursive_with_ignore(
    root: &Path,
    path: &Path,
    parent_id: Option<&LocalFileId>,
    nodes: &mut Vec<LocalNode>,
    inode_to_id: &mut HashMap<(u64, u64), LocalFileId>,
    rules: &IgnoreRules,
) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        let Ok(metadata) = fs::symlink_metadata(&entry_path) else {
            continue;
        };

        if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
            tracing::debug!(path = %entry_path.display(), "⏭️ Skipping non-regular file");
            continue;
        }

        let is_dir = metadata.is_dir();

        // Compute relative path for ignore check
        let rel_path = entry_path
            .strip_prefix(root)
            .unwrap_or(&entry_path)
            .to_string_lossy();
        if rules.is_ignored(&rel_path, is_dir) {
            tracing::debug!(path = %entry_path.display(), "⏭️ Skipping ignored path");
            continue;
        }

        // ... rest is identical to scan_recursive: build the node, push it, recurse for dirs
        // (copy the body from scan_recursive, replacing the recursive call with scan_recursive_with_ignore)
    }
    Ok(())
}
```

The body after the ignore check is identical to the existing `scan_recursive` — compute inode, name, type/md5/size, TOCTOU check, push node, recurse for directories. Use `scan_recursive_with_ignore` for the recursive call, passing `root` and `rules` through.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p super-ragondin-sync test_scan_ignores`
Expected: all tests pass

**Step 5: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 6: Commit**

```bash
git add crates/sync/src/local/scanner.rs crates/sync/tests/local_tests.rs
git commit -m "feat: scanner skips ignored files and directories"
```

---

### Task 5: Integrate ignore rules into the `SyncEngine`

**Files:**
- Modify: `crates/sync/src/sync/engine.rs`

The `SyncEngine` needs to hold an `IgnoreRules` and use `scan_with_ignore` instead of `scan` in `initial_scan`. This also means filtering remote changes: when a remote node's path is ignored, skip inserting it.

**Step 1: Write the failing test**

Add to `crates/sync/tests/sync_tests.rs`:

```rust
#[test]
fn ignored_local_files_are_not_uploaded() {
    let dir = tempdir().unwrap();
    let sync_dir = dir.path().join("sync");
    fs::create_dir_all(&sync_dir).unwrap();

    fs::write(sync_dir.join("normal.txt"), b"hello").unwrap();
    fs::write(sync_dir.join(".hidden"), b"secret").unwrap();

    let store = TreeStore::open(&dir.path().join("store")).unwrap();
    let rules = IgnoreRules::default_only();
    let mut engine = SyncEngine::new(
        store,
        sync_dir.clone(),
        dir.path().join("staging"),
        rules,
    );
    engine.initial_scan().unwrap();
    let results = engine.plan().unwrap();

    // Only normal.txt should produce a CreateRemoteFile op, not .hidden
    let create_ops: Vec<_> = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(SyncOp::CreateRemoteFile { .. })))
        .collect();
    assert_eq!(create_ops.len(), 1);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-sync ignored_local_files_are_not_uploaded`
Expected: compilation error (SyncEngine::new expects different arguments)

**Step 3: Add `IgnoreRules` to `SyncEngine`**

In `crates/sync/src/sync/engine.rs`:

1. Add field `rules: IgnoreRules` to the `SyncEngine` struct.
2. Update `new()` to accept `rules: IgnoreRules` and store it.
3. In `initial_scan()`, replace `scanner.scan()?` with `scanner.scan_with_ignore(&self.rules)?`.
4. In `fetch_and_apply_remote_changes()`, before inserting a remote node, compute its relative path and check `self.rules.is_ignored(...)`. If ignored, skip the node. Use the existing `compute_remote_rel_path`-style logic, or a simpler approach: check the node's name against the rules (this catches most patterns like `.*`, `*.tmp`, etc.). For full path matching, you'd need the parent chain — but since the planner already does this, the simplest approach is to filter at the planner level (see Task 6).

```rust
pub struct SyncEngine {
    store: TreeStore,
    sync_dir: PathBuf,
    staging_dir: PathBuf,
    rules: IgnoreRules,
}

impl SyncEngine {
    #[must_use]
    pub const fn new(
        store: TreeStore,
        sync_dir: PathBuf,
        staging_dir: PathBuf,
        rules: IgnoreRules,
    ) -> Self {
        Self { store, sync_dir, staging_dir, rules }
    }
    // ...
}
```

**Step 4: Update all call sites**

Update `crates/cli/src/main.rs` `open_engine()`:

```rust
fn open_engine(config: &Config) -> Result<SyncEngine> {
    let store = TreeStore::open(&config.store_dir())?;
    let rules = IgnoreRules::load(Some(&config.data_dir.join("syncignore")));
    Ok(SyncEngine::new(
        store,
        config.sync_dir.clone(),
        config.staging_dir(),
        rules,
    ))
}
```

Update all `SyncEngine::new(...)` calls in test files (`sync_tests.rs`, `integration_tests.rs`) to pass `IgnoreRules::default_only()` as the 4th argument. This keeps existing tests working with default rules (which won't affect them since test files use names like `hello.txt`).

**Step 5: Run all tests**

Run: `cargo test -q`
Expected: all tests pass

**Step 6: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 7: Commit**

```bash
git add crates/sync/src/sync/engine.rs crates/cli/src/main.rs crates/sync/tests/
git commit -m "feat: SyncEngine holds IgnoreRules, scanner uses them"
```

---

### Task 6: Filter ignored nodes in the planner

**Files:**
- Modify: `crates/sync/src/planner.rs`
- Modify: `crates/sync/tests/sync_tests.rs`

The planner should skip remote nodes whose full relative path is ignored. This is the second checkpoint: even if a remote change slips into the store, the planner won't generate operations for it.

**Step 1: Write the failing test**

Add to `crates/sync/tests/sync_tests.rs`:

```rust
#[test]
fn planner_skips_ignored_remote_nodes() {
    let dir = tempdir().unwrap();
    let sync_dir = dir.path().join("sync");
    fs::create_dir_all(&sync_dir).unwrap();

    let store = TreeStore::open(&dir.path().join("store")).unwrap();

    // Bootstrap root
    let root_remote_id = RemoteId::new("io.cozy.files.root-dir");
    store
        .insert_remote_node(&RemoteNode {
            id: root_remote_id.clone(),
            parent_id: None,
            name: String::new(),
            node_type: NodeType::Directory,
            md5sum: None,
            size: None,
            updated_at: 0,
            rev: String::new(),
        })
        .unwrap();

    // Insert a remote node named ".hidden-file"
    store
        .insert_remote_node(&RemoteNode {
            id: RemoteId::new("hidden-id"),
            parent_id: Some(root_remote_id.clone()),
            name: ".hidden-file".to_string(),
            node_type: NodeType::File,
            md5sum: Some("abc123".to_string()),
            size: Some(10),
            updated_at: 1000,
            rev: "1-abc".to_string(),
        })
        .unwrap();

    // Insert a normal remote node
    store
        .insert_remote_node(&RemoteNode {
            id: RemoteId::new("normal-id"),
            parent_id: Some(root_remote_id),
            name: "normal.txt".to_string(),
            node_type: NodeType::File,
            md5sum: Some("def456".to_string()),
            size: Some(20),
            updated_at: 1000,
            rev: "1-def".to_string(),
        })
        .unwrap();

    store.flush().unwrap();

    let rules = IgnoreRules::default_only();
    let planner = Planner::new(&store, sync_dir, rules);
    let results = planner.plan().unwrap();

    // Should only plan for normal.txt, not .hidden-file
    let create_ops: Vec<_> = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(SyncOp::CreateLocalFile { .. })))
        .collect();
    assert_eq!(create_ops.len(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-sync planner_skips_ignored_remote_nodes`
Expected: fails (Planner::new signature mismatch or the hidden file generates an op)

**Step 3: Add `IgnoreRules` to `Planner`**

1. Add `rules: IgnoreRules` field (owned, not borrowed — simpler lifetime).
   Actually, use `rules: &'a IgnoreRules` to match the existing `store: &'a TreeStore` pattern.
2. Update `Planner::new()` to accept `rules: &'a IgnoreRules`.
3. In the `plan()` method, after computing `compute_remote_rel_path(remote)`, check `self.rules.is_ignored(rel_path, is_dir)`. If ignored, skip the remote node (emit a `NoOp` or simply `continue`).
4. Similarly, in the `plan_local_only()` call loop, compute the local rel path and skip if ignored.

In `plan()`, inside the remote nodes loop (around line 58), add before `plan_remote_node`:

```rust
let rel_path = self.compute_remote_rel_path(remote);
let rel_str = rel_path.to_string_lossy();
if !rel_str.is_empty() && self.rules.is_ignored(&rel_str, remote.node_type == NodeType::Directory) {
    tracing::debug!(path = %rel_str, "⏭️ Skipping ignored remote node");
    continue;
}
```

In the local-only loop (around line 91), add before `plan_local_only`:

```rust
let rel_path = self.compute_local_rel_path(local);
let rel_str = rel_path.to_string_lossy();
if !rel_str.is_empty() && self.rules.is_ignored(&rel_str, local.node_type == NodeType::Directory) {
    tracing::debug!(path = %rel_str, "⏭️ Skipping ignored local node");
    continue;
}
```

**Step 4: Update `SyncEngine` to pass rules to planner**

In `SyncEngine::plan()`:

```rust
pub fn plan(&self) -> Result<Vec<PlanResult>> {
    let planner = Planner::new(&self.store, self.sync_dir.clone(), &self.rules);
    planner.plan()
}
```

**Step 5: Run all tests**

Run: `cargo test -q`
Expected: all tests pass

**Step 6: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 7: Commit**

```bash
git add crates/sync/src/planner.rs crates/sync/src/sync/engine.rs crates/sync/tests/sync_tests.rs
git commit -m "feat: planner skips ignored local and remote nodes"
```

---

### Task 7: Filter ignored paths in the watcher

**Files:**
- Modify: `crates/sync/src/local/watcher.rs`
- Modify: `crates/sync/tests/local_tests.rs`

The watcher should skip events for ignored paths. This avoids triggering unnecessary re-scans and is the earliest filtering checkpoint.

**Step 1: Write the failing test**

Add to `crates/sync/tests/local_tests.rs`:

```rust
#[test]
fn test_watcher_ignores_hidden_file_events() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let rules = IgnoreRules::default_only();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx, rules).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

    // Create an ignored file — should NOT produce an event
    fs::write(root.join(".hidden"), b"secret").unwrap();
    // Create a normal file — SHOULD produce an event
    fs::write(root.join("visible.txt"), b"hello").unwrap();

    // Collect events for a short window
    let mut events = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(2)) {
        events.push(event);
    }

    // Only visible.txt events should appear
    assert!(events.iter().all(|e| !e.path.to_string_lossy().contains(".hidden")));
    assert!(events.iter().any(|e| e.path.to_string_lossy().contains("visible")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-sync test_watcher_ignores`
Expected: compilation error (`Watcher::new` signature mismatch)

**Step 3: Add ignore filtering to the watcher**

1. Add `rules: IgnoreRules` field to `Watcher` and `root` for relative path computation.
2. Update `Watcher::new()` to accept `rules: IgnoreRules`.
3. In `add_watch_recursive()`, skip directories whose relative path is ignored.
4. In `run()`, before sending the event, compute the relative path and check `self.rules.is_ignored(...)`. If ignored, `continue`.

In `run()`, before the `let watch_event = ...` block:

```rust
let rel_path = path.strip_prefix(&self.root).unwrap_or(&path);
let rel_str = rel_path.to_string_lossy();
if !rel_str.is_empty() && self.rules.is_ignored(&rel_str, is_dir) {
    tracing::debug!(path = %path.display(), "⏭️ Watcher: skipping ignored path");
    continue;
}
```

**Step 4: Update all `Watcher::new()` call sites**

In `crates/cli/src/main.rs`, pass the rules to the watcher. The engine owns the rules, so either:
- Make `IgnoreRules` implement `Clone` (it wraps `Gitignore` which is `Clone`), or
- Create the rules before the engine and share via `Arc<IgnoreRules>`.

Simplest: derive/implement `Clone` on `IgnoreRules`, create it once in the CLI, clone for watcher and engine.

Update existing watcher tests to pass `IgnoreRules::default_only()`.

**Step 5: Run all tests**

Run: `cargo test -q`
Expected: all tests pass

**Step 6: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 7: Commit**

```bash
git add crates/sync/src/local/watcher.rs crates/sync/src/ignore.rs crates/cli/src/main.rs crates/sync/tests/local_tests.rs
git commit -m "feat: watcher filters out events for ignored paths"
```

---

### Task 8: Add user syncignore file support to the CLI

**Files:**
- Modify: `crates/cli/src/main.rs`

**Step 1: Use the data dir syncignore path**

The `open_engine` function already loads from `config.data_dir.join("syncignore")` (from Task 5). Add a `syncignore_path()` helper to `Config`:

In `crates/sync/src/config.rs`:

```rust
#[must_use]
pub fn syncignore_path(&self) -> PathBuf {
    self.data_dir.join("syncignore")
}
```

Then in `open_engine`:

```rust
let rules = IgnoreRules::load(Some(&config.syncignore_path()));
```

**Step 2: Run all tests**

Run: `cargo test -q`
Expected: all tests pass

**Step 3: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 4: Commit**

```bash
git add crates/sync/src/config.rs crates/cli/src/main.rs
git commit -m "feat: load user syncignore from data directory"
```

---

### Task 9: Update the simulator (if it creates its own planner)

**Files:**
- Modify: `crates/sync/src/simulator/runner.rs` (if it constructs `Planner` directly)

**Step 1: Check if the simulator constructs a `Planner`**

Search for `Planner::new` in `simulator/runner.rs`. If it does, it needs to pass `IgnoreRules` too.

**Step 2: Update to pass `IgnoreRules::default_only()` (or an empty-rules variant)**

For the simulator, we probably want no ignore rules so all generated files participate in sync. Add an `IgnoreRules::none()` constructor that matches nothing:

In `crates/sync/src/ignore.rs`:

```rust
/// Create rules that ignore nothing (for testing/simulation).
#[must_use]
pub fn none() -> Self {
    let builder = GitignoreBuilder::new("");
    let gitignore = builder.build().expect("failed to build empty gitignore");
    Self { gitignore }
}
```

Pass `IgnoreRules::none()` in the simulator's planner construction.

**Step 3: Run all tests including proptests**

Run: `cargo test -q`
Expected: all tests pass

**Step 4: Run formatter and linter**

```bash
cargo fmt --all
cargo clippy --all-features
```

**Step 5: Commit**

```bash
git add crates/sync/src/simulator/runner.rs crates/sync/src/ignore.rs
git commit -m "feat: simulator uses no-op ignore rules"
```

---

### Task 10: Final verification

**Step 1: Run the full test suite**

```bash
cargo test -q
```

**Step 2: Run clippy**

```bash
cargo clippy --all-features
```

**Step 3: Run formatter**

```bash
cargo fmt --all
```

**Step 4: Manual smoke test (optional)**

Create a test directory with ignored and normal files, run a sync cycle, and verify ignored files are not synced:

```bash
mkdir -p /tmp/test-sync
echo "hello" > /tmp/test-sync/normal.txt
echo "secret" > /tmp/test-sync/.hidden
echo "temp" > /tmp/test-sync/file.swp
# Run super-ragondin sync and verify only normal.txt is uploaded
```
