# saveFile and listDirs JS Tools Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `saveFile(path, content, options?)` and `listDirs(prefix?)` JavaScript globals to the code-mode sandbox, allowing the LLM agent to write files into the local sync directory and discover its directory structure.

**Architecture:** Both tools are Boa `NativeFunction` globals registered in a fresh JS context per sandbox execution. They access the sync directory via `SANDBOX_CTX` (a thread-local set before each `Sandbox::execute()` call). Path traversal is prevented by a component-walk check (no filesystem access needed).

**Tech Stack:** Rust, Boa engine (JS sandbox), `base64` crate (new), `std::fs` (file I/O), `tempfile` (tests already in dev-deps).

**Spec:** `docs/specs/2026-03-13-save-file-list-dirs-design.md`

> **Note on commits:** Per project rules, do not commit automatically. Commit steps below describe what to stage — commit manually when you judge the change is stable.

---

## Chunk 1: Dependency and sandbox plumbing

### Task 1: Add base64 dependency

**Files:**
- Modify: `crates/codemode/Cargo.toml`

- [ ] **Step 1: Add the dependency**

```bash
cargo add base64 -p super-ragondin-codemode
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p super-ragondin-codemode
```

Expected: compiles without errors.

- [ ] **Step 3: Stage for commit**

Stage: `crates/codemode/Cargo.toml`, `Cargo.lock`

Suggested message: `chore(codemode): add base64 dependency for saveFile binary encoding`

---

### Task 2: Add sync_dir to SandboxContext and Sandbox

The `SANDBOX_CTX` thread-local holds shared state for all tool functions. We add `sync_dir: PathBuf` to it and to the `Sandbox` struct that sets it up.

> **Note:** After this task, `crates/codemode` will compile but `crates/cli` will fail because `CodeModeEngine::new` still calls the old 2-argument `Sandbox::new`. That is fixed in Task 6 (Chunk 3). Build the codemode crate only here.

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Write failing tests**

In `crates/codemode/src/sandbox.rs`, update the `#[cfg(test)]` block.

Replace the existing `make_sandbox()` helper. Note: return all three `TempDir` values to prevent early drop of the database directory:

```rust
async fn make_sandbox() -> (Sandbox, tempfile::TempDir, tempfile::TempDir) {
    let db_dir = tempfile::tempdir()
        .expect("failed to create temp db dir");
    let sync_dir = tempfile::tempdir()
        .expect("failed to create temp sync dir");
    let store = Arc::new(
        RagStore::open(db_dir.path()).await
            .expect("failed to open RagStore"),
    );
    let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    let sandbox = Sandbox::new(store, config, sync_dir.path().to_path_buf());
    (sandbox, db_dir, sync_dir)
}
```

Update all existing tests that call `make_sandbox()` to destructure all three values. For tests that don't need the sync dir, bind with `_`:

```rust
let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
```

- [ ] **Step 2: Run tests — they should fail to compile**

```bash
cargo test -p super-ragondin-codemode -q 2>&1 | head -30
```

Expected: compile error — `Sandbox::new` still takes 2 args, `make_sandbox` return type mismatch.

- [ ] **Step 3: Update SandboxContext**

In `crates/codemode/src/sandbox.rs`, add `sync_dir` to `SandboxContext`:

```rust
#[allow(dead_code)]
pub struct SandboxContext {
    pub store: Arc<RagStore>,
    pub embedder: Arc<OpenRouterEmbedder>,
    pub config: RagConfig,
    pub handle: tokio::runtime::Handle,
    pub sync_dir: std::path::PathBuf,
}
```

- [ ] **Step 4: Update Sandbox struct and constructor**

Change `Sandbox` struct (add `sync_dir`) and change `new` from `const fn` to `fn`:

```rust
#[allow(dead_code)]
pub struct Sandbox {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
}

#[allow(dead_code)]
impl Sandbox {
    /// Create a new sandbox with the given store, config, and sync directory.
    #[must_use]
    pub fn new(store: Arc<RagStore>, config: RagConfig, sync_dir: std::path::PathBuf) -> Self {
        Self { store, config, sync_dir }
    }
```

- [ ] **Step 5: Pass sync_dir into SandboxContext inside execute()**

In `Sandbox::execute()`, add `sync_dir` to the `SandboxContext` construction:

```rust
SANDBOX_CTX.with(|cell| {
    *cell.borrow_mut() = Some(SandboxContext {
        store: Arc::clone(&self.store),
        embedder,
        config: self.config.clone(),
        handle,
        sync_dir: self.sync_dir.clone(),
    });
});
```

- [ ] **Step 6: Build codemode only — verify it compiles**

```bash
cargo build -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

Expected: clean. (The CLI crate will break — fixed in Task 6.)

- [ ] **Step 7: Format and lint (codemode only)**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

Fix any errors.

- [ ] **Step 8: Stage for commit**

Stage: `crates/codemode/src/sandbox.rs`

Suggested message: `feat(codemode): add sync_dir to SandboxContext and Sandbox`

---

## Chunk 2: save_file and list_dirs tools

### Task 3: Implement save_file.rs

**Files:**
- Create: `crates/codemode/src/tools/save_file.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

- [ ] **Step 1: Write the failing tests — registration and path validation**

Create `crates/codemode/src/tools/save_file.rs` with a stub and tests:

```rust
use std::path::{Component, Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::SANDBOX_CTX;

fn check_relative_path(path: &str) -> Result<(), &'static str> {
    todo!()
}

/// Register the `saveFile(path, content, options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    todo!()
}

fn save_file_fn(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof saveFile"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_check_relative_path_rejects_parent_dir() {
        assert!(check_relative_path("../etc/passwd").is_err());
        assert!(check_relative_path("notes/../../../etc").is_err());
        assert!(check_relative_path("a/b/../../..").is_err());
    }

    #[test]
    fn test_check_relative_path_accepts_normal_paths() {
        assert!(check_relative_path("notes/summary.md").is_ok());
        assert!(check_relative_path("./notes/file.txt").is_ok());
        assert!(check_relative_path("file.txt").is_ok());
        assert!(check_relative_path("a/b/c").is_ok());
    }
}
```

- [ ] **Step 2: Add modules to mod.rs**

In `crates/codemode/src/tools/mod.rs`, add both new modules (create a stub `list_dirs.rs` in the next task — add the declaration now so the plan flows cleanly, but it will fail to compile until Task 4 creates the file):

```rust
pub mod get_document;
pub mod list_dirs;
pub mod list_files;
pub mod save_file;
pub mod search;
pub mod sub_agent;
```

Create a minimal `crates/codemode/src/tools/list_dirs.rs` stub now so the crate compiles:

```rust
use boa_engine::{Context, JsError, JsValue, NativeFunction, js_string};

#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(js_string!("listDirs"), 0, NativeFunction::from_fn_ptr(|_, _, _| todo!()))?;
    Ok(())
}
```

- [ ] **Step 3: Run tests — verify they fail (todo! panics)**

```bash
cargo test -p super-ragondin-codemode tools::save_file -q 2>&1 | head -20
```

Expected: test failure from `todo!()` panics.

- [ ] **Step 4: Implement check_relative_path**

```rust
fn check_relative_path(path: &str) -> Result<(), &'static str> {
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir | Component::RootDir => {
                return Err("path escapes sync directory");
            }
            _ => {}
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Implement register**

```rust
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("saveFile"),
        0,
        NativeFunction::from_fn_ptr(save_file_fn),
    )?;
    Ok(())
}
```

- [ ] **Step 6: Implement save_file_fn**

```rust
fn save_file_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use boa_engine::JsArgs;

    let path = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();

    let content = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();

    let encoding = args
        .get(2)
        .and_then(|opts| opts.as_object())
        .and_then(|o| o.get(boa_engine::JsString::from("encoding"), ctx).ok())
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|| "utf8".to_string());

    if Path::new(&path).is_absolute() {
        return Err(JsNativeError::error()
            .with_message("path must be relative")
            .into());
    }

    if let Err(msg) = check_relative_path(&path) {
        return Err(JsNativeError::error().with_message(msg).into());
    }

    let sync_dir = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<PathBuf, JsError>(sandbox.sync_dir.clone())
    })?;

    let target = sync_dir.join(&path);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
    }

    let bytes: Vec<u8> = if encoding == "base64" {
        STANDARD
            .decode(&content)
            .map_err(|_| JsNativeError::error().with_message("invalid base64 content"))?
    } else {
        content.into_bytes()
    };

    std::fs::write(&target, &bytes)
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;

    Ok(JsValue::undefined())
}
```

- [ ] **Step 7: Run the unit tests — they should pass**

```bash
cargo test -p super-ragondin-codemode tools::save_file -q
```

Expected: 3 tests pass.

- [ ] **Step 8: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

- [ ] **Step 9: Stage for commit**

Stage: `crates/codemode/src/tools/save_file.rs`, `crates/codemode/src/tools/mod.rs`

Suggested message: `feat(codemode): add saveFile JS global for writing files to sync directory`

---

### Task 4: Implement list_dirs.rs

**Files:**
- Modify: `crates/codemode/src/tools/list_dirs.rs` (replace the stub from Task 3)

- [ ] **Step 1: Write the failing tests in list_dirs.rs**

Replace the stub with tests + stubs:

```rust
use std::path::{Component, Path, PathBuf};

use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

fn check_relative_path(path: &str) -> Result<(), &'static str> {
    todo!()
}

/// Register the `listDirs(prefix?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    todo!()
}

fn list_dirs_fn(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof listDirs"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_check_relative_path_rejects_parent_dir() {
        assert!(check_relative_path("../secret").is_err());
        assert!(check_relative_path("a/../../../b").is_err());
    }

    #[test]
    fn test_check_relative_path_accepts_normal_paths() {
        assert!(check_relative_path("work").is_ok());
        assert!(check_relative_path("work/projects").is_ok());
        assert!(check_relative_path("./work").is_ok());
    }
}
```

- [ ] **Step 2: Run tests — verify they fail (todo! panics)**

```bash
cargo test -p super-ragondin-codemode tools::list_dirs -q 2>&1 | head -20
```

Expected: test failures from `todo!()`.

- [ ] **Step 3: Implement the full list_dirs.rs**

Replace the file with the complete implementation:

```rust
use std::path::{Component, Path, PathBuf};

use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};

use crate::sandbox::{SANDBOX_CTX, serde_to_jsvalue};

fn check_relative_path(path: &str) -> Result<(), &'static str> {
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir | Component::RootDir => {
                return Err("path escapes sync directory");
            }
            _ => {}
        }
    }
    Ok(())
}

/// Register the `listDirs(prefix?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("listDirs"),
        0,
        NativeFunction::from_fn_ptr(list_dirs_fn),
    )?;
    Ok(())
}

fn list_dirs_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let prefix = args
        .first()
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();

    if !prefix.is_empty() {
        if let Err(msg) = check_relative_path(&prefix) {
            return Err(JsNativeError::error().with_message(msg).into());
        }
    }

    let sync_dir = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<PathBuf, JsError>(sandbox.sync_dir.clone())
    })?;

    let target = if prefix.is_empty() {
        sync_dir
    } else {
        sync_dir.join(&prefix)
    };

    if !target.exists() {
        return serde_to_jsvalue(&serde_json::Value::Array(vec![]), ctx);
    }

    if !target.is_dir() {
        return Err(JsNativeError::error()
            .with_message("not a directory")
            .into());
    }

    let mut dirs: Vec<String> = std::fs::read_dir(&target)
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_dir() {
                entry.file_name().into_string().ok()
            } else {
                None
            }
        })
        .collect();

    dirs.sort();

    let json_dirs: Vec<serde_json::Value> =
        dirs.into_iter().map(serde_json::Value::String).collect();
    serde_to_jsvalue(&serde_json::Value::Array(json_dirs), ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registers_without_panic() {
        let mut ctx = boa_engine::Context::default();
        register(&mut ctx).unwrap();
        let result = ctx.eval(boa_engine::Source::from_bytes(b"typeof listDirs"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_check_relative_path_rejects_parent_dir() {
        assert!(check_relative_path("../secret").is_err());
        assert!(check_relative_path("a/../../../b").is_err());
    }

    #[test]
    fn test_check_relative_path_accepts_normal_paths() {
        assert!(check_relative_path("work").is_ok());
        assert!(check_relative_path("work/projects").is_ok());
        assert!(check_relative_path("./work").is_ok());
    }
}
```

- [ ] **Step 4: Run the unit tests — they should pass**

```bash
cargo test -p super-ragondin-codemode tools::list_dirs -q
```

Expected: 3 tests pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

- [ ] **Step 6: Stage for commit**

Stage: `crates/codemode/src/tools/list_dirs.rs`

Suggested message: `feat(codemode): add listDirs JS global for discovering sync directory structure`

---

### Task 5: Register new tools in the sandbox and add functional tests

The `Sandbox::run_boa()` method registers all tools. We register `saveFile` and `listDirs` and add functional integration tests via `sandbox.execute()` in `sandbox.rs`.

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Write failing functional tests in sandbox.rs**

Add these tests to the `#[cfg(test)]` block in `sandbox.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_writes_utf8_default_encoding() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    let result = sandbox.execute(r#"saveFile("hello.txt", "world")"#).unwrap();
    assert_eq!(result, "null");
    let content = std::fs::read_to_string(sync_dir.path().join("hello.txt"))
        .expect("file should exist");
    assert_eq!(content, "world");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_writes_utf8_explicit_encoding() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    sandbox
        .execute(r#"saveFile("note.md", "hello", { encoding: "utf8" })"#)
        .unwrap();
    let content = std::fs::read_to_string(sync_dir.path().join("note.md"))
        .expect("file should exist");
    assert_eq!(content, "hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_writes_base64_binary() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    // "AQID" is base64 for bytes [1, 2, 3]
    sandbox
        .execute(r#"saveFile("data.bin", "AQID", { encoding: "base64" })"#)
        .unwrap();
    let bytes = std::fs::read(sync_dir.path().join("data.bin")).expect("file should exist");
    assert_eq!(bytes, vec![1u8, 2, 3]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_creates_parent_dirs() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    sandbox
        .execute(r#"saveFile("a/b/c/file.txt", "content")"#)
        .unwrap();
    let content = std::fs::read_to_string(sync_dir.path().join("a/b/c/file.txt"))
        .expect("file should exist");
    assert_eq!(content, "content");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_rejects_traversal() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let err = sandbox.execute(r#"saveFile("../escape.txt", "x")"#).unwrap_err();
    assert!(err.contains("path escapes sync directory"), "got: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_rejects_absolute_path() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let err = sandbox.execute(r#"saveFile("/etc/passwd", "x")"#).unwrap_err();
    assert!(err.contains("path must be relative"), "got: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_save_file_rejects_invalid_base64() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let err = sandbox
        .execute(r#"saveFile("f.bin", "!!!not-base64!!!", { encoding: "base64" })"#)
        .unwrap_err();
    assert!(err.contains("invalid base64 content"), "got: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_empty_sync_dir() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute("listDirs()").unwrap();
    assert_eq!(result, "[]");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_returns_only_directories() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    std::fs::create_dir(sync_dir.path().join("notes"))
        .expect("failed to create dir");
    std::fs::create_dir(sync_dir.path().join("work"))
        .expect("failed to create dir");
    std::fs::write(sync_dir.path().join("file.txt"), "x")
        .expect("failed to write file");
    let result = sandbox.execute("listDirs()").unwrap();
    let dirs: Vec<String> = serde_json::from_str(&result).unwrap();
    assert_eq!(dirs, vec!["notes", "work"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_nested_prefix() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    std::fs::create_dir_all(sync_dir.path().join("work/projects"))
        .expect("failed to create dirs");
    std::fs::create_dir_all(sync_dir.path().join("work/meetings"))
        .expect("failed to create dirs");
    let result = sandbox.execute(r#"listDirs("work")"#).unwrap();
    let dirs: Vec<String> = serde_json::from_str(&result).unwrap();
    assert_eq!(dirs, vec!["meetings", "projects"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_nonexistent_prefix_returns_empty() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute(r#"listDirs("nonexistent")"#).unwrap();
    assert_eq!(result, "[]");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_file_as_prefix_returns_error() {
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
    std::fs::write(sync_dir.path().join("file.txt"), "x")
        .expect("failed to write file");
    let err = sandbox.execute(r#"listDirs("file.txt")"#).unwrap_err();
    assert!(err.contains("not a directory"), "got: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_dirs_rejects_traversal() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let err = sandbox.execute(r#"listDirs("../escape")"#).unwrap_err();
    assert!(err.contains("path escapes sync directory"), "got: {err}");
}
```

- [ ] **Step 2: Run — verify tests fail (saveFile/listDirs not yet registered in run_boa)**

```bash
cargo test -p super-ragondin-codemode sandbox -q 2>&1 | tail -20
```

Expected: test failures — `saveFile` and `listDirs` are `undefined` or `todo!` panics.

- [ ] **Step 3: Update test_sandbox_globals_registered to include the new functions**

In the `#[cfg(test)]` block in `sandbox.rs`, update `test_sandbox_globals_registered`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_sandbox_globals_registered() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    for fn_name in &["search", "listFiles", "getDocument", "subAgent", "saveFile", "listDirs"] {
        let result = sandbox.execute(&format!("typeof {fn_name}")).unwrap();
        assert_eq!(
            result,
            format!("\"function\""),
            "{fn_name} should be a function"
        );
    }
}
```

- [ ] **Step 4: Register the new tools in run_boa()**

In `sandbox.rs`, in `fn run_boa()`, add after the existing `register` calls:

```rust
tools::save_file::register(&mut ctx)
    .map_err(|e| format!("JS error: register saveFile: {e}"))?;
tools::list_dirs::register(&mut ctx)
    .map_err(|e| format!("JS error: register listDirs: {e}"))?;
```

- [ ] **Step 5: Run all codemode tests — they should pass**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all tests pass including `test_sandbox_globals_registered` and all new functional tests.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

- [ ] **Step 7: Stage for commit**

Stage: `crates/codemode/src/sandbox.rs`

Suggested message: `feat(codemode): register saveFile and listDirs in sandbox, add functional tests`

---

## Chunk 3: Wiring, prompt, and docs

> **Prerequisites (completed in Chunks 1–2):**
> - `SandboxContext` and `Sandbox` have `sync_dir: PathBuf` (Task 2)
> - `Sandbox::new` takes 3 args: `(store, config, sync_dir)` (Task 2)
> - `make_sandbox()` returns `(Sandbox, TempDir, TempDir)` (Task 2)
> - `tools::save_file` and `tools::list_dirs` are registered in `run_boa()` (Task 5)

### Task 6: Update CodeModeEngine to accept sync_dir

**Files:**
- Modify: `crates/codemode/src/engine.rs`

- [ ] **Step 1: Add sync_dir to CodeModeEngine**

In `engine.rs`, update the struct:

```rust
pub struct CodeModeEngine {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
}
```

Update the constructor:

```rust
pub async fn new(config: RagConfig, sync_dir: std::path::PathBuf) -> Result<Self> {
    let store = Arc::new(RagStore::open(&config.db_path).await?);
    Ok(Self { store, config, sync_dir })
}
```

- [ ] **Step 2: Pass sync_dir into spawn_blocking**

In the `ask()` method's tool-call loop, add `sync_dir_clone` alongside the existing clones:

```rust
let store_clone = Arc::clone(&self.store);
let config_clone = self.config.clone();
let sync_dir_clone = self.sync_dir.clone();
let code_clone = tool_call.code.clone();
let id_clone = tool_call.id.clone();
handles.push(tokio::task::spawn_blocking(move || {
    let sandbox = Sandbox::new(store_clone, config_clone, sync_dir_clone);
    (id_clone, sandbox.execute(&code_clone))
}));
```

- [ ] **Step 3: Update execute_js tool description**

Replace the description string in `execute_js_tool_definition()`:

Old:
```
"Use the search(), listFiles(), getDocument(), and subAgent() functions to query the document database."
```

New:
```
"Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), and listDirs() functions to query the document database and write files."
```

- [ ] **Step 4: Build codemode only — verify it compiles**

```bash
cargo build -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

Expected: clean. (CLI still broken until Task 8.)

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

- [ ] **Step 6: Stage for commit**

Stage: `crates/codemode/src/engine.rs`

Suggested message: `feat(codemode): add sync_dir to CodeModeEngine, update execute_js description`

---

### Task 7: Update prompt.rs

**Files:**
- Modify: `crates/codemode/src/prompt.rs`

- [ ] **Step 1: Write the failing test**

In `prompt.rs`, update `test_prompt_contains_key_elements`:

```rust
#[test]
fn test_prompt_contains_key_elements() {
    let p = system_prompt();
    assert!(p.contains("Super Ragondin"));
    assert!(p.contains("execute_js"));
    assert!(p.contains("search("));
    assert!(p.contains("listFiles("));
    assert!(p.contains("getDocument("));
    assert!(p.contains("subAgent("));
    assert!(p.contains("saveFile("));
    assert!(p.contains("listDirs("));
    assert!(p.contains("ISO 8601"));
}
```

- [ ] **Step 2: Run — verify it fails**

```bash
cargo test -p super-ragondin-codemode prompt -q
```

Expected: FAIL — `saveFile(` and `listDirs(` not found in prompt.

- [ ] **Step 3: Update the system prompt**

In `system_prompt()`, add to the "Available JavaScript functions" section (after `subAgent`):

```
  saveFile(path, content, options?)
    Write a file into the sync directory. options: { encoding: "utf8" | "base64" }
    Default encoding is "utf8". Use "base64" for binary content.
    Creates intermediate directories automatically.
    Returns: null

  listDirs(prefix?)
    Non-recursive: list only immediate subdirectory names at a given path within the sync directory.
    Returns: string[] — directory names only, sorted alphabetically
```

Add to the Examples section:

```
// Discover top-level directories
listDirs()

// Explore a subdirectory before saving
const dirs = listDirs("work");
// dirs might be ["meetings", "projects"]

// Save a text summary
saveFile("notes/summary.md", "# Summary\n\nKey points...", { encoding: "utf8" })

// Save a generated image (base64)
saveFile("images/chart.png", base64EncodedPngString, { encoding: "base64" })
```

- [ ] **Step 4: Run — verify tests pass**

```bash
cargo test -p super-ragondin-codemode prompt -q
```

Expected: PASS.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
```

- [ ] **Step 6: Stage for commit**

Stage: `crates/codemode/src/prompt.rs`

Suggested message: `docs(codemode): document saveFile and listDirs in system prompt`

---

### Task 8: Update CLI call site

**Files:**
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Build workspace to see the compile error**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: error in `crates/cli/src/main.rs` — `CodeModeEngine::new` called with 1 argument, now requires 2.

- [ ] **Step 2: Fix cmd_ask to pass sync_dir**

In `cmd_ask`, `config` (of type `Config`) is already loaded and has `config.sync_dir: PathBuf`. Update the call:

```rust
let engine = CodeModeEngine::new(rag_config, config.sync_dir.clone())
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
```

- [ ] **Step 3: Build workspace — verify clean**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: clean build.

- [ ] **Step 4: Run all tests**

```bash
cargo test -q
```

Expected: all tests pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features 2>&1 | grep -E "^error"
```

- [ ] **Step 6: Stage for commit**

Stage: `crates/cli/src/main.rs`

Suggested message: `feat(cli): pass sync_dir to CodeModeEngine for saveFile/listDirs support`

---

### Task 9: Update AGENTS.md

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Add the new tool files to the codemode section**

In `AGENTS.md`, find the `crates/codemode/` section and add after the existing tool entries:

```
  - `src/tools/save_file.rs` - `saveFile(path, content, options?)` JS global — write files to the sync directory (utf8 or base64 encoding)
  - `src/tools/list_dirs.rs` - `listDirs(prefix?)` JS global — list immediate subdirectories in the sync directory
```

- [ ] **Step 2: Stage for commit**

Stage: `AGENTS.md`

Suggested message: `docs: update AGENTS.md with saveFile and listDirs tools`

---

### Task 10: Final verification

- [ ] **Step 1: Run the full test suite**

```bash
cargo test -q
```

Expected: all tests pass, no failures.

- [ ] **Step 2: Run clippy on the full workspace**

```bash
cargo clippy --all-features 2>&1 | grep -E "^(error|warning\[)"
```

Expected: no errors, no new warnings.

- [ ] **Step 3: Format check**

```bash
cargo fmt --all -- --check
```

Expected: exits 0 (no formatting issues).
