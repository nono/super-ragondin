# GUI Ask Assistant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single "Synchronizing" screen in the Tauri GUI with a two-column layout — sync status on the left, RAG-powered ask assistant on the right.

**Architecture:** The `Ready` state in `App.svelte` renders a new `MainLayout.svelte` (two-column flex) instead of `Syncing.svelte`. Three new Tauri commands (`ask_question`, `get_suggestions`, `get_recent_files`) are backed by `CodeModeEngine` and `SuggestionEngine` from `crates/codemode`. `CodeModeEngine::ask()` is changed to return `Result<String>` instead of printing. The OpenRouter API key moves from env-var-only into `Config`.

**Tech Stack:** Rust, Tauri v2, tauri-specta, Svelte 5, `super-ragondin-codemode`, `super-ragondin-rag`, `super-ragondin-sync`

---

## File Map

| Action | File | What changes |
|---|---|---|
| Modify | `crates/gui/Cargo.toml` | Add `super-ragondin-rag` and `super-ragondin-codemode` deps |
| Modify | `crates/sync/src/config.rs` | Add `#[serde(default)] api_key: Option<String>` to `Config` |
| Modify | `crates/codemode/src/engine.rs` | `ask()` returns `Result<String>` instead of printing |
| Modify | `crates/cli/src/main.rs` | Print the `String` returned by `engine.ask()` |
| Modify | `crates/gui/src/commands.rs` | Add `api_key` to `init_config`/`init_config_to`; add three new commands |
| Modify | `crates/gui/tauri.conf.json` | Resize window to 760×520, resizable, min 600×400 |
| Modify | `gui-frontend/src/bindings.ts` | Regenerated — do not edit by hand |
| Modify | `gui-frontend/src/App.svelte` | Switch to light theme; render `MainLayout` for `Ready` state |
| Modify | `gui-frontend/src/lib/Setup.svelte` | Add OpenRouter API key input field |
| Create | `gui-frontend/src/lib/MainLayout.svelte` | Two-column flex wrapper |
| Create | `gui-frontend/src/lib/SyncPanel.svelte` | Left column: sync badge + recent files list |
| Create | `gui-frontend/src/lib/AskPanel.svelte` | Right column: suggestions → asking → done state machine |

---

## Task 1: Add codemode and rag dependencies to the GUI crate

**Files:**
- Modify: `crates/gui/Cargo.toml`

- [ ] **Step 1: Add both crates as dependencies**

```bash
cd crates/gui
cargo add super-ragondin-rag --path ../rag
cargo add super-ragondin-codemode --path ../codemode
```

- [ ] **Step 2: Verify the GUI crate still builds**

```bash
cargo build -p super-ragondin-gui
```

Expected: compiles without errors (warnings are OK).

- [ ] **Step 3: Commit**

```bash
git add crates/gui/Cargo.toml Cargo.lock
git commit -m "feat(gui): add rag and codemode crate dependencies"
```

---

## Task 2: Add `api_key` to `Config`

**Files:**
- Modify: `crates/sync/src/config.rs`

- [ ] **Step 1: Write two failing tests**

Add to the `#[cfg(test)]` block in `crates/sync/src/config.rs` (alongside the existing `save_sets_owner_only_permissions` test):

```rust
#[test]
fn api_key_round_trips_through_save_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut config = test_config();
    config.api_key = Some("sk-test-key".to_string());
    config.save(&path).unwrap();
    let loaded = Config::load(&path).unwrap().unwrap();
    assert_eq!(loaded.api_key, Some("sk-test-key".to_string()));
}

#[test]
fn old_config_without_api_key_loads_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    // Write a JSON object that has no api_key field (simulates existing config)
    std::fs::write(
        &path,
        r#"{"instance_url":"https://x.mycozy.cloud","sync_dir":"/tmp/sync","data_dir":"/tmp/data","oauth_client":null,"last_seq":null}"#,
    ).unwrap();
    let loaded = Config::load(&path).unwrap().unwrap();
    assert_eq!(loaded.api_key, None);
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test -p super-ragondin-sync api_key
```

Expected: FAIL — `Config` has no `api_key` field.

- [ ] **Step 3: Add the field to `Config`**

In `crates/sync/src/config.rs`, add to the `Config` struct after `last_seq`:

```rust
#[serde(default)]
pub api_key: Option<String>,
```

Also update `test_config()` helper in the same file to include `api_key: None`:

```rust
fn test_config() -> Config {
    Config {
        instance_url: "https://test.mycozy.cloud".to_string(),
        sync_dir: PathBuf::from("/tmp/sync"),
        data_dir: PathBuf::from("/tmp/data"),
        oauth_client: Some(OAuthClient { /* ... existing ... */ }),
        last_seq: None,
        api_key: None,
    }
}
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -p super-ragondin-sync api_key
```

Expected: 2 tests PASS.

- [ ] **Step 5: Fix all other Config struct literals in the codebase**

The `Config { ... }` struct literals in `crates/gui/src/commands.rs` (test helpers around line 432) and `crates/cli/src/main.rs` (around line 66) must also be updated to include `api_key: None`. Find them:

```bash
grep -rn "Config {" crates/
```

Add `api_key: None,` to each one.

- [ ] **Step 6: Run the full test suite to confirm nothing is broken**

```bash
cargo test -q
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/sync/src/config.rs crates/gui/src/commands.rs crates/cli/src/main.rs
git commit -m "feat(config): add api_key field with serde default for backward compat"
```

---

## Task 3: Change `CodeModeEngine::ask()` to return `Result<String>`

**Files:**
- Modify: `crates/codemode/src/engine.rs`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Update the existing unit test to assert the returned string**

In `crates/codemode/src/engine.rs`, find the test `test_extract_text_from_response` (around line 404). It verifies `extract_text` but doesn't test `ask()` return directly. Instead, find any call to `engine.ask(...)` in tests. There may not be direct tests of `ask()` (it requires live OpenRouter). In that case, skip writing a new test for `ask()` itself — the compile error from the CLI will serve as the integration check. Proceed to the implementation step.

- [ ] **Step 2: Update `ask()` return type and body**

In `crates/codemode/src/engine.rs`:

Change the signature from:
```rust
pub async fn ask(&self, question: &str, context_dir: Option<std::path::PathBuf>) -> Result<()> {
```
to:
```rust
pub async fn ask(&self, question: &str, context_dir: Option<std::path::PathBuf>) -> Result<String> {
```

Replace `println!("{text}"); return Ok(())` with `return Ok(text)`.

Replace the final `Ok(())` at the bottom of the function (after the `for` block) with `unreachable!()`. The `bail!` inside the loop on the last iteration always fires first, so this line is never reached — `unreachable!()` makes that explicit.

- [ ] **Step 3: Update `cmd_ask` in the CLI to print the returned string**

In `crates/cli/src/main.rs`, in `cmd_ask`, find the call to `engine.ask(&question, cwd).await`:

```rust
engine
    .ask(&question, cwd)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))
```

Change to:

```rust
let answer = engine
    .ask(&question, cwd)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
println!("{answer}");
Ok(())
```

(Remove the `?` at the end of the block and handle the `Result` explicitly.)

- [ ] **Step 4: Build to confirm no compile errors**

```bash
cargo build -p super-ragondin -p super-ragondin-codemode
```

Expected: compiles without errors.

- [ ] **Step 5: Run codemode tests**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/engine.rs crates/cli/src/main.rs
git commit -m "refactor(codemode): ask() returns String instead of printing to stdout"
```

---

## Task 4: Update `init_config` / `init_config_to` to accept `api_key`

**Files:**
- Modify: `crates/gui/src/commands.rs`

- [ ] **Step 1: Write a failing test**

Add to the `#[cfg(test)]` block in `crates/gui/src/commands.rs`:

```rust
#[test]
fn init_config_to_saves_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let sync_dir = dir.path().join("sync");
    let config_path = dir.path().join("config.json");
    let result = init_config_to(
        "https://alice.mycozy.cloud".to_string(),
        sync_dir.to_str().unwrap().to_string(),
        Some("sk-openrouter-test".to_string()),
        &config_path,
    );
    assert!(result.is_ok());
    let loaded = super_ragondin_sync::config::Config::load(&config_path)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.api_key, Some("sk-openrouter-test".to_string()));
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p super-ragondin-gui init_config_to_saves_api_key
```

Expected: FAIL — `init_config_to` doesn't accept `api_key` yet.

- [ ] **Step 3: Update `init_config_to` and `init_config`**

In `crates/gui/src/commands.rs`:

Change `init_config_to` signature:
```rust
pub fn init_config_to(
    instance_url: String,
    sync_dir: String,
    api_key: Option<String>,
    config_path: &std::path::Path,
) -> Result<(), String> {
```

Inside `init_config_to`, add `api_key` to the `Config` struct literal:
```rust
let config = Config {
    instance_url,
    sync_dir: sync_dir.clone(),
    data_dir: data_dir.clone(),
    oauth_client: None,
    last_seq: None,
    api_key,
};
```

Change the Tauri command `init_config`:
```rust
#[tauri::command]
#[specta::specta]
pub fn init_config(instance_url: String, sync_dir: String, api_key: Option<String>) -> Result<(), String> {
    init_config_to(instance_url, sync_dir, api_key, &config_path())
}
```

Update the existing test `init_config_creates_dirs_and_config` to pass `None` for `api_key`:
```rust
let result = init_config_to(
    instance_url.clone(),
    sync_dir.to_str().unwrap().to_string(),
    None,
    &dir.path().join("config.json"),
);
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -p super-ragondin-gui -q
```

Expected: all pass.

- [ ] **Step 5: Regenerate TypeScript bindings**

```bash
cargo test -p super-ragondin-gui --ignored -- export_bindings
```

Expected: `gui-frontend/src/bindings.ts` updated — `initConfig` now takes a third argument.

- [ ] **Step 6: Commit**

```bash
git add crates/gui/src/commands.rs gui-frontend/src/bindings.ts
git commit -m "feat(gui): add api_key parameter to init_config"
```

---

## Task 5: Add `get_recent_files` command

**Files:**
- Modify: `crates/gui/src/commands.rs`

- [ ] **Step 1: Write a failing test**

Add to `#[cfg(test)]` in `crates/gui/src/commands.rs`:

```rust
#[test]
fn get_recent_files_returns_top_files_by_mtime() {
    use super_ragondin_cozy_client::types::{NodeType, RemoteId};
    use super_ragondin_sync::model::{LocalFileId, SyncedRecord};
    use super_ragondin_sync::store::TreeStore;

    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join("store");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).unwrap();

    // Write two files to disk with a small mtime gap
    let file_a = sync_dir.join("a.txt");
    let file_b = sync_dir.join("b.txt");
    std::fs::write(&file_a, "a").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file_b, "b").unwrap();

    // Insert matching SyncedRecords into the store
    let store = TreeStore::open(&store_dir).unwrap();
    let record_a = SyncedRecord {
        local_id: LocalFileId::new(1, 1),
        remote_id: RemoteId::new("ra"),
        rel_path: "a.txt".to_string(),
        md5sum: None,
        size: None,
        rev: "1-a".to_string(),
        node_type: NodeType::File,
        local_name: None,
        local_parent_id: None,
        remote_name: None,
        remote_parent_id: None,
    };
    let record_b = SyncedRecord {
        local_id: LocalFileId::new(1, 2),
        remote_id: RemoteId::new("rb"),
        rel_path: "b.txt".to_string(),
        md5sum: None,
        size: None,
        rev: "1-b".to_string(),
        node_type: NodeType::File,
        local_name: None,
        local_parent_id: None,
        remote_name: None,
        remote_parent_id: None,
    };
    store.insert_synced(&record_a).unwrap();
    store.insert_synced(&record_b).unwrap();

    let result = get_recent_files_from(&store_dir, &sync_dir);
    assert!(result.is_ok());
    let files = result.unwrap();
    assert_eq!(files.len(), 2);
    // b.txt was written later so it must appear first
    assert_eq!(files[0], "b.txt");
    assert_eq!(files[1], "a.txt");
}
```

> `get_recent_files_from` is the testable inner function you'll extract (see Step 3). `LocalFileId::new(device_id, inode)` — use distinct inode values per record.

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p super-ragondin-gui get_recent_files
```

Expected: FAIL — function doesn't exist yet.

- [ ] **Step 3: Implement `get_recent_files_from` and the Tauri command**

Add to `crates/gui/src/commands.rs`:

```rust
/// Testable core of `get_recent_files`: opens the store, stats each file, returns top 10.
pub fn get_recent_files_from(
    store_dir: &std::path::Path,
    sync_dir: &std::path::Path,
) -> Result<Vec<String>, String> {
    use super_ragondin_sync::model::NodeType;
    use super_ragondin_sync::store::TreeStore;

    let store = TreeStore::open(store_dir).map_err(|e| e.to_string())?;
    let synced = store.list_all_synced().map_err(|e| e.to_string())?;

    let mut entries: Vec<(std::time::SystemTime, String)> = synced
        .into_iter()
        .filter(|r| r.node_type == NodeType::File)
        .filter_map(|r| {
            let abs = sync_dir.join(&r.rel_path);
            // Silently skip files that no longer exist on disk
            let mtime = std::fs::metadata(&abs).ok()?.modified().ok()?;
            Some((mtime, r.rel_path.clone()))
        })
        .collect();

    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(entries.into_iter().take(10).map(|(_, p)| p).collect())
}

#[tauri::command]
#[specta::specta]
pub fn get_recent_files() -> Result<Vec<String>, String> {
    let config = Config::load(&config_path())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;
    get_recent_files_from(&config.store_dir(), &config.sync_dir)
}
```

Register in `make_builder`:

```rust
.commands(tauri_specta::collect_commands![
    get_app_state,
    init_config,
    start_auth,
    start_sync,
    get_recent_files,  // add this
])
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-ragondin-gui -q
```

Expected: all pass.

- [ ] **Step 5: Regenerate bindings**

```bash
cargo test -p super-ragondin-gui --ignored -- export_bindings
```

Expected: `bindings.ts` now includes `getRecentFiles`.

- [ ] **Step 6: Commit**

```bash
git add crates/gui/src/commands.rs gui-frontend/src/bindings.ts
git commit -m "feat(gui): add get_recent_files command"
```

---

## Task 6: Add `get_suggestions` command

**Files:**
- Modify: `crates/gui/src/commands.rs`

- [ ] **Step 1: Write failing tests**

Add to `#[cfg(test)]` in `crates/gui/src/commands.rs`:

```rust
#[tokio::test]
async fn get_suggestions_no_api_key_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    // Save a config with no api_key
    let config = super_ragondin_sync::config::Config {
        instance_url: "https://x.mycozy.cloud".to_string(),
        sync_dir: dir.path().join("sync"),
        data_dir: dir.path().into(),
        oauth_client: None,
        last_seq: None,
        api_key: None,
    };
    config.save(&config_path).unwrap();

    let result = get_suggestions_from(&config_path).await;
    assert_eq!(result, Err("NoApiKey".to_string()));
}

#[tokio::test]
async fn get_suggestions_no_files_indexed_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config = super_ragondin_sync::config::Config {
        instance_url: "https://x.mycozy.cloud".to_string(),
        sync_dir: dir.path().join("sync"),
        data_dir: dir.path().into(),
        oauth_client: None,
        last_seq: None,
        api_key: Some("sk-test".to_string()),
    };
    std::fs::create_dir_all(config.rag_dir()).unwrap();
    config.save(&config_path).unwrap();

    let result = get_suggestions_from(&config_path).await;
    assert_eq!(result, Err("NoFilesIndexed".to_string()));
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test -p super-ragondin-gui get_suggestions
```

Expected: FAIL.

- [ ] **Step 3: Implement `get_suggestions_from` and the Tauri command**

```rust
/// Testable core: loads config from `config_path`, runs SuggestionEngine.
pub async fn get_suggestions_from(
    config_path: &std::path::Path,
) -> Result<Vec<String>, String> {
    use super_ragondin_codemode::suggestions::{NoFilesIndexed, SuggestionEngine};
    use super_ragondin_rag::config::RagConfig;

    let config = Config::load(config_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;

    let api_key = config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "NoApiKey".to_string())?
        .to_string();

    let mut rag_config = RagConfig::from_env_with_db_path(config.rag_dir());
    rag_config.api_key = api_key;

    let engine = SuggestionEngine::new(rag_config, config.sync_dir)
        .await
        .map_err(|e| e.to_string())?;

    engine.generate(None).await.map_err(|e| {
        if e.downcast_ref::<NoFilesIndexed>().is_some() {
            "NoFilesIndexed".to_string()
        } else {
            e.to_string()
        }
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_suggestions() -> Result<Vec<String>, String> {
    get_suggestions_from(&config_path()).await
}
```

Register in `make_builder`:

```rust
.commands(tauri_specta::collect_commands![
    get_app_state,
    init_config,
    start_auth,
    start_sync,
    get_recent_files,
    get_suggestions,  // add this
])
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-ragondin-gui -q
```

Expected: all pass.

- [ ] **Step 5: Regenerate bindings**

```bash
cargo test -p super-ragondin-gui --ignored -- export_bindings
```

- [ ] **Step 6: Commit**

```bash
git add crates/gui/src/commands.rs gui-frontend/src/bindings.ts
git commit -m "feat(gui): add get_suggestions command"
```

---

## Task 7: Add `ask_question` command

**Files:**
- Modify: `crates/gui/src/commands.rs`

- [ ] **Step 1: Write a failing test**

```rust
#[tokio::test]
async fn ask_question_no_api_key_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config = super_ragondin_sync::config::Config {
        instance_url: "https://x.mycozy.cloud".to_string(),
        sync_dir: dir.path().join("sync"),
        data_dir: dir.path().into(),
        oauth_client: None,
        last_seq: None,
        api_key: None,
    };
    config.save(&config_path).unwrap();

    let result = ask_question_from("What is in my files?", &config_path).await;
    assert_eq!(result, Err("NoApiKey".to_string()));
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p super-ragondin-gui ask_question_no_api_key
```

Expected: FAIL.

- [ ] **Step 3: Implement `ask_question_from` and the Tauri command**

```rust
/// Testable core: loads config from `config_path`, runs CodeModeEngine.
pub async fn ask_question_from(
    question: &str,
    config_path: &std::path::Path,
) -> Result<String, String> {
    use super_ragondin_codemode::engine::CodeModeEngine;
    use super_ragondin_rag::config::RagConfig;

    let config = Config::load(config_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No config".to_string())?;

    let api_key = config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "NoApiKey".to_string())?
        .to_string();

    let mut rag_config = RagConfig::from_env_with_db_path(config.rag_dir());
    rag_config.api_key = api_key;

    let engine = CodeModeEngine::new(rag_config, config.sync_dir, None)
        .await
        .map_err(|e| e.to_string())?;

    engine.ask(question, None).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn ask_question(question: String) -> Result<String, String> {
    ask_question_from(&question, &config_path()).await
}
```

Register in `make_builder`:

```rust
.commands(tauri_specta::collect_commands![
    get_app_state,
    init_config,
    start_auth,
    start_sync,
    get_recent_files,
    get_suggestions,
    ask_question,   // add this
])
```

Also add to `collect_events` and the event list in `make_builder` — events stay unchanged, only commands list grows.

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-ragondin-gui -q
```

Expected: all pass.

- [ ] **Step 5: Regenerate bindings**

```bash
cargo test -p super-ragondin-gui --ignored -- export_bindings
```

- [ ] **Step 6: Run clippy and fmt**

```bash
cargo fmt --all
cargo clippy --all-features
```

Fix any warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/gui/src/commands.rs gui-frontend/src/bindings.ts
git commit -m "feat(gui): add ask_question command"
```

---

## Task 8: Resize the window

**Files:**
- Modify: `crates/gui/tauri.conf.json`

- [ ] **Step 1: Update window config**

Replace the `windows` section in `crates/gui/tauri.conf.json`:

```json
"windows": [
  {
    "title": "Super Ragondin",
    "width": 760,
    "height": 520,
    "minWidth": 600,
    "minHeight": 400,
    "resizable": true,
    "center": true
  }
]
```

- [ ] **Step 2: Build and visually verify window size (manual check)**

```bash
cd crates/gui && cargo tauri dev
```

Expected: window opens at ~760×520 and can be resized down to 600×400.

- [ ] **Step 3: Commit**

```bash
git add crates/gui/tauri.conf.json
git commit -m "feat(gui): resize window to 760x520 resizable"
```

---

## Task 9: Update `Setup.svelte` with API key field

**Files:**
- Modify: `gui-frontend/src/lib/Setup.svelte`

- [ ] **Step 1: Add `apiKey` state, new input field, and update the command call**

Replace the content of `gui-frontend/src/lib/Setup.svelte` with:

```svelte
<script lang="ts">
  import { commands } from '../bindings'

  interface Props {
    oncomplete: () => void
  }

  const { oncomplete }: Props = $props()

  let instanceUrl: string = $state('')
  let syncDir: string = $state('')
  let apiKey: string = $state('')
  let error: string | null = $state(null)
  let submitting: boolean = $state(false)

  async function handleSubmit() {
    submitting = true
    error = null
    try {
      const result = await commands.initConfig(instanceUrl, syncDir, apiKey || null)
      if (result.status === 'error') {
        error = result.error
      } else {
        oncomplete()
      }
    } catch (e) {
      error = String(e)
    } finally {
      submitting = false
    }
  }
</script>

<div class="container">
  <h1>Super Ragondin</h1>
  <form onsubmit={(e) => { e.preventDefault(); handleSubmit() }}>
    <label>
      Cozy instance URL
      <input
        type="url"
        bind:value={instanceUrl}
        placeholder="https://alice.mycozy.cloud"
        required
      />
    </label>
    <label>
      Sync directory
      <input
        type="text"
        bind:value={syncDir}
        placeholder="/home/user/Cozy"
        required
      />
    </label>
    <label>
      OpenRouter API key
      <input
        type="password"
        bind:value={apiKey}
        placeholder="sk-or-…"
      />
    </label>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <button type="submit" disabled={submitting}>
      {submitting ? 'Saving…' : 'Connect to Cozy →'}
    </button>
  </form>
</div>

<style>
  .container {
    width: 380px;
    padding: 24px;
  }
  h1 {
    font-size: 18px;
    margin-bottom: 20px;
    text-align: center;
    color: #333;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: #666;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  input {
    background: #fff;
    border: 1px solid #ccc;
    border-radius: 4px;
    padding: 8px 10px;
    color: #333;
    font-size: 14px;
  }
  input:focus {
    outline: none;
    border-color: #2f80ed;
  }
  button {
    background: #2f80ed;
    color: #fff;
    border: none;
    border-radius: 4px;
    padding: 10px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    margin-top: 4px;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .error {
    color: #d32f2f;
    font-size: 13px;
  }
</style>
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd gui-frontend && npm run build
```

Expected: builds without type errors.

- [ ] **Step 3: Commit**

```bash
git add gui-frontend/src/lib/Setup.svelte
git commit -m "feat(gui): add OpenRouter API key field to setup form"
```

---

## Task 10: Update `App.svelte` and create `MainLayout.svelte`

**Files:**
- Modify: `gui-frontend/src/App.svelte`
- Create: `gui-frontend/src/lib/MainLayout.svelte`

- [ ] **Step 1: Create `MainLayout.svelte`**

Create `gui-frontend/src/lib/MainLayout.svelte`:

```svelte
<script lang="ts">
  import SyncPanel from './SyncPanel.svelte'
  import AskPanel from './AskPanel.svelte'
</script>

<div class="layout">
  <SyncPanel />
  <AskPanel />
</div>

<style>
  .layout {
    display: flex;
    width: 100%;
    height: 100vh;
    overflow: hidden;
  }
</style>
```

- [ ] **Step 2: Update `App.svelte`**

Replace `Syncing` import and usage with `MainLayout`, and switch the global theme to light:

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from './bindings'
  import type { AppState } from './bindings'
  import Setup from './lib/Setup.svelte'
  import Auth from './lib/Auth.svelte'
  import MainLayout from './lib/MainLayout.svelte'

  let appState: AppState | null = $state(null)
  let authError: string | null = $state(null)

  let unlistenAuthComplete: (() => void) | undefined
  let unlistenAuthError: (() => void) | undefined

  onMount(async () => {
    unlistenAuthComplete = await events.authCompleteEvent.listen(() => {
      appState = 'Ready'
      authError = null
    })
    unlistenAuthError = await events.authErrorEvent.listen((event) => {
      authError = event.payload.message
    })
    appState = await commands.getAppState()
  })

  onDestroy(() => {
    unlistenAuthComplete?.()
    unlistenAuthError?.()
  })

  function handleSetupComplete() {
    authError = null
    appState = 'Unauthenticated'
  }
</script>

<main>
  {#if appState === null}
    <div class="loading">Loading…</div>
  {:else if appState === 'Unconfigured'}
    <Setup oncomplete={handleSetupComplete} />
  {:else if appState === 'Unauthenticated'}
    <Auth bind:error={authError} />
  {:else if appState === 'Ready'}
    <MainLayout />
  {/if}
</main>

<style>
  :global(*, *::before, *::after) {
    box-sizing: border-box;
    margin: 0;
    padding: 0;
  }
  :global(body) {
    font-family: system-ui, sans-serif;
    font-size: 14px;
    background: #f5f5f0;
    color: #333;
  }
  main {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .loading {
    color: #888;
  }
</style>
```

- [ ] **Step 3: Verify it builds (SyncPanel and AskPanel don't exist yet — expect compile errors)**

```bash
cd gui-frontend && npm run build 2>&1 | head -20
```

Expected: errors about missing `SyncPanel` and `AskPanel` — that's fine, next tasks will add them.

- [ ] **Step 4: Commit**

```bash
git add gui-frontend/src/App.svelte gui-frontend/src/lib/MainLayout.svelte
git commit -m "feat(gui): add MainLayout two-column shell, switch to light theme"
```

---

## Task 11: Create `SyncPanel.svelte`

**Files:**
- Create: `gui-frontend/src/lib/SyncPanel.svelte`

- [ ] **Step 1: Create the component**

Create `gui-frontend/src/lib/SyncPanel.svelte`:

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from '../bindings'
  import type { SyncState } from '../bindings'

  let status: SyncState = $state('Idle')
  let lastSync: string | null = $state(null)
  let recentFiles: string[] = $state([])

  let unlistenSyncStatus: (() => void) | undefined

  async function refreshRecentFiles() {
    const result = await commands.getRecentFiles()
    if (result.status === 'ok') {
      recentFiles = result.data
    }
    // On error: silently keep existing list
  }

  onMount(async () => {
    unlistenSyncStatus = await events.syncStatusEvent.listen(async (event) => {
      status = event.payload.status
      lastSync = event.payload.last_sync
      if (status === 'Idle') {
        await refreshRecentFiles()
      }
    })
    commands.startSync()
    await refreshRecentFiles()
  })

  onDestroy(() => {
    unlistenSyncStatus?.()
  })

  function formatLastSync(iso: string | null): string {
    if (!iso) return 'Never'
    try {
      return new Date(iso).toLocaleString()
    } catch {
      return iso
    }
  }

  function fileIcon(path: string): string {
    const ext = path.split('.').pop()?.toLowerCase() ?? ''
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'].includes(ext)) return '🖼'
    if (['pdf'].includes(ext)) return '📕'
    if (['md', 'txt', 'csv'].includes(ext)) return '📄'
    return '📝'
  }

  function fileName(path: string): string {
    return path.split('/').pop() ?? path
  }
</script>

<div class="panel">
  <div class="app-title">Super Ragondin</div>

  <div class="status-badge" class:syncing={status === 'Syncing'}>
    <span class="dot"></span>
    <span class="label">{status === 'Syncing' ? 'Syncing…' : 'Up to date'}</span>
  </div>

  <div class="section">
    <div class="section-title">Recent files</div>
    {#if recentFiles.length === 0}
      <p class="empty">No files yet</p>
    {:else}
      <ul class="file-list">
        {#each recentFiles as file}
          <li class="file-item">
            <span class="file-icon">{fileIcon(file)}</span>
            <span class="file-name" title={file}>{fileName(file)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <p class="last-sync">Last sync: {formatLastSync(lastSync)}</p>
</div>

<style>
  .panel {
    width: 220px;
    flex-shrink: 0;
    background: #efefea;
    border-right: 1px solid #ddd;
    display: flex;
    flex-direction: column;
    padding: 16px 14px;
    gap: 12px;
    overflow-y: auto;
  }
  .app-title {
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: #888;
    text-transform: uppercase;
  }
  .status-badge {
    display: flex;
    align-items: center;
    gap: 7px;
    background: #e4f4e4;
    border: 1px solid #b5dbb5;
    border-radius: 6px;
    padding: 7px 10px;
  }
  .status-badge.syncing {
    background: #fff4e0;
    border-color: #f0c060;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4caf50;
    flex-shrink: 0;
  }
  .status-badge.syncing .dot {
    background: #f0a020;
  }
  .label {
    font-size: 12px;
    color: #2e7d2e;
    font-weight: 600;
  }
  .status-badge.syncing .label {
    color: #a06010;
  }
  .section-title {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: #aaa;
    margin-bottom: 6px;
  }
  .file-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .file-item {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 7px;
    border-radius: 5px;
    background: #fff;
    border: 1px solid #e8e8e3;
  }
  .file-icon {
    font-size: 12px;
    flex-shrink: 0;
  }
  .file-name {
    font-size: 11px;
    color: #444;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    font-size: 11px;
    color: #aaa;
  }
  .last-sync {
    font-size: 10px;
    color: #aaa;
    margin-top: auto;
  }
</style>
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd gui-frontend && npm run build 2>&1 | head -20
```

Expected: `SyncPanel` error gone; only `AskPanel` missing now.

- [ ] **Step 3: Commit**

```bash
git add gui-frontend/src/lib/SyncPanel.svelte
git commit -m "feat(gui): add SyncPanel with sync status and recent files list"
```

---

## Task 12: Create `AskPanel.svelte`

**Files:**
- Create: `gui-frontend/src/lib/AskPanel.svelte`

> **Note on Markdown rendering:** The spec calls for answers rendered as Markdown. For this initial version the answer is displayed with `white-space: pre-wrap` (raw text with whitespace preserved). Adding a Markdown renderer (e.g. `marked`) is deferred — the panel is structured so it can be swapped in later by replacing the `{answer}` binding with a Markdown component.

- [ ] **Step 1: Create the component**

Create `gui-frontend/src/lib/AskPanel.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte'
  import { commands } from '../bindings'

  type PanelState = 'loading' | 'no-api-key' | 'idle' | 'asking' | 'done' | 'error'

  let state: PanelState = $state('loading')
  let suggestions: string[] = $state([])
  let question: string = $state('')
  let lastQuestion: string = $state('')
  let answer: string = $state('')
  let errorMessage: string = $state('')

  onMount(async () => {
    await loadSuggestions()
  })

  async function loadSuggestions() {
    state = 'loading'
    const result = await commands.getSuggestions()
    if (result.status === 'ok') {
      suggestions = result.data
      state = 'idle'
    } else if (result.error === 'NoApiKey') {
      state = 'no-api-key'
    } else if (result.error === 'NoFilesIndexed') {
      suggestions = []
      state = 'idle'
    } else {
      suggestions = []
      state = 'idle'
    }
  }

  async function ask(q: string) {
    if (!q.trim()) return
    lastQuestion = q
    question = ''
    state = 'asking'
    const result = await commands.askQuestion(q)
    if (result.status === 'ok') {
      answer = result.data
      state = 'done'
    } else {
      errorMessage = result.error
      state = 'error'
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      ask(question)
    }
  }
</script>

<div class="panel">
  <div class="panel-header">
    <span class="icon">✦</span>
    <span class="title">Ask</span>
  </div>

  <div class="panel-body">
    {#if state === 'loading'}
      <p class="hint">Loading suggestions…</p>

    {:else if state === 'no-api-key'}
      <div class="banner">
        Add your OpenRouter API key during setup to use the assistant.
      </div>

    {:else if state === 'idle'}
      {#if suggestions.length > 0}
        <p class="hint">Not sure what to ask? Here are some ideas:</p>
        <ul class="chips">
          {#each suggestions as s}
            <li>
              <button class="chip" onclick={() => ask(s)}>
                <span class="chip-arrow">↗</span> {s}
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="hint">No files indexed yet — waiting for first sync.</p>
      {/if}

    {:else if state === 'asking'}
      <div class="message user">{lastQuestion}</div>
      <div class="thinking">
        <span class="dot"></span><span class="dot"></span><span class="dot"></span>
        Thinking…
      </div>

    {:else if state === 'done'}
      <div class="message user">{lastQuestion}</div>
      <div class="message assistant">{answer}</div>

    {:else if state === 'error'}
      <div class="message user">{lastQuestion}</div>
      <div class="message error-msg">{errorMessage}</div>
    {/if}
  </div>

  {#if state !== 'no-api-key'}
    <div class="input-row">
      <input
        type="text"
        bind:value={question}
        placeholder={state === 'done' || state === 'error' ? 'Ask another question…' : 'Ask anything about your files…'}
        disabled={state === 'asking' || state === 'loading'}
        onkeydown={handleKeydown}
      />
      <button
        class="send-btn"
        onclick={() => ask(question)}
        disabled={state === 'asking' || state === 'loading' || !question.trim()}
      >
        Ask
      </button>
    </div>
  {/if}
</div>

<style>
  .panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    background: #fff;
    min-width: 0;
  }
  .panel-header {
    padding: 12px 16px 10px;
    border-bottom: 1px solid #ebebeb;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .icon { font-size: 14px; }
  .title { font-size: 13px; font-weight: 600; color: #333; }

  .panel-body {
    flex: 1;
    padding: 16px 18px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .hint { font-size: 12px; color: #888; }

  .banner {
    background: #fff3e0;
    border: 1px solid #ffe0b2;
    border-radius: 6px;
    padding: 10px 14px;
    font-size: 12px;
    color: #e65100;
  }

  .chips {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .chip {
    width: 100%;
    text-align: left;
    padding: 9px 13px;
    background: #f7f7f3;
    border: 1px solid #e0e0d8;
    border-radius: 8px;
    font-size: 12px;
    color: #444;
    cursor: pointer;
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .chip:hover { background: #eeeee8; }
  .chip-arrow { color: #aaa; font-size: 11px; flex-shrink: 0; }

  .message {
    padding: 9px 13px;
    border-radius: 8px;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .message.user {
    background: #2f80ed;
    color: #fff;
    align-self: flex-end;
    border-bottom-right-radius: 3px;
    max-width: 88%;
  }
  .message.assistant {
    background: #f3f3ee;
    color: #333;
    border: 1px solid #e8e8e3;
    border-bottom-left-radius: 3px;
    align-self: flex-start;
    max-width: 95%;
  }
  .message.error-msg {
    background: #fff0f0;
    color: #c62828;
    border: 1px solid #ffcdd2;
    border-bottom-left-radius: 3px;
    align-self: flex-start;
  }

  .thinking {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: #aaa;
    padding: 8px 0;
  }
  .thinking .dot {
    width: 5px;
    height: 5px;
    background: #ccc;
    border-radius: 50%;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .thinking .dot:nth-child(2) { animation-delay: 0.2s; }
  .thinking .dot:nth-child(3) { animation-delay: 0.4s; }
  @keyframes pulse {
    0%, 80%, 100% { opacity: 0.3; transform: scale(0.8); }
    40% { opacity: 1; transform: scale(1); }
  }

  .input-row {
    padding: 10px 14px;
    border-top: 1px solid #ebebeb;
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .input-row input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 12px;
    color: #333;
    background: #fafafa;
    outline: none;
    font-family: inherit;
  }
  .input-row input:focus { border-color: #2f80ed; }
  .input-row input::placeholder { color: #bbb; }
  .input-row input:disabled { opacity: 0.5; }
  .send-btn {
    padding: 8px 14px;
    background: #2f80ed;
    color: #fff;
    border: none;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }
  .send-btn:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
```

- [ ] **Step 2: Verify the full frontend build passes**

```bash
cd gui-frontend && npm run build
```

Expected: builds without errors.

- [ ] **Step 3: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy --all-features
```

Fix any warnings.

- [ ] **Step 4: Run the full test suite**

```bash
cargo test -q
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add gui-frontend/src/lib/AskPanel.svelte
git commit -m "feat(gui): add AskPanel with suggestions, asking, and done states"
```

---

## Done

All tasks complete. The GUI now shows a two-column light-theme layout:
- Left: sync status badge + recent files list
- Right: ask assistant with AI suggestions on idle, thinking spinner during queries, and answer display on completion

To test end-to-end manually:
```bash
cd crates/gui && cargo tauri dev
```
