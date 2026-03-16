# Scratchpad Tool Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `remember(key, value)` and `recall(key)` as JS globals in the codemode sandbox, backed by an `Arc<Mutex<HashMap>>` shared across all `execute_js` calls within a single `ask()` session.

**Architecture:** A `Scratchpad` type alias (`Arc<Mutex<HashMap<String, serde_json::Value>>>`) is created once per `ask()` call in the engine and threaded down into each `Sandbox` instance. Each sandbox stores it in the `SandboxContext` thread-local before calling `run_boa`, making it available to the `remember`/`recall` native callbacks via `SANDBOX_CTX`.

**Tech Stack:** Rust, Boa JS engine (`boa_engine`), `serde_json`, `std::sync::{Arc, Mutex}`.

---

## Chunk 1: scratchpad.rs + sandbox wiring + engine wiring

### Task 1: Create `scratchpad.rs` with type and registration (failing test first)

**Files:**
- Create: `crates/codemode/src/tools/scratchpad.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

- [ ] **Step 1: Add the module declaration (makes the next step fail to compile)**

  In `crates/codemode/src/tools/mod.rs`, add after the last `pub mod` line:

  ```rust
  pub mod scratchpad;
  ```

- [ ] **Step 2: Run to verify it fails to compile**

  ```bash
  cargo test -q -p super-ragondin-codemode 2>&1 | head -20
  ```

  Expected: compile error — module file `scratchpad.rs` not found.

- [ ] **Step 3: Implement `scratchpad.rs`**

  Create `crates/codemode/src/tools/scratchpad.rs` with the full content (implementation + tests together):

  ```rust
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};

  use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
  use serde_json::Value as SerdeValue;

  use crate::sandbox::{SANDBOX_CTX, jsvalue_to_serde, serde_to_jsvalue};

  /// Shared in-session key-value store, valid for one `ask()` call.
  pub type Scratchpad = Arc<Mutex<HashMap<String, SerdeValue>>>;

  /// Create a fresh, empty scratchpad for a new `ask()` session.
  #[must_use]
  pub fn new_scratchpad() -> Scratchpad {
      Arc::new(Mutex::new(HashMap::new()))
  }

  /// Register `remember(key, value)` and `recall(key)` as JS globals.
  ///
  /// # Errors
  /// Returns error if the global function cannot be registered.
  #[allow(dead_code)]
  pub fn register(ctx: &mut Context) -> Result<(), JsError> {
      ctx.register_global_callable(
          js_string!("remember"),
          2,
          NativeFunction::from_fn_ptr(remember_fn),
      )?;
      ctx.register_global_callable(
          js_string!("recall"),
          1,
          NativeFunction::from_fn_ptr(recall_fn),
      )?;
      Ok(())
  }

  fn remember_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
      use boa_engine::JsArgs;

      let key = args
          .get_or_undefined(0)
          .to_string(ctx)?
          .to_std_string_escaped();

      let value = jsvalue_to_serde(args.get_or_undefined(1).clone(), ctx);

      let scratchpad = SANDBOX_CTX.with(|cell| {
          let borrow = cell.borrow();
          let sandbox = borrow.as_ref().ok_or_else(|| {
              JsNativeError::error().with_message("sandbox context not initialized")
          })?;
          Ok::<Scratchpad, JsError>(Arc::clone(&sandbox.scratchpad))
      })?;

      scratchpad
          .lock()
          .map_err(|_| JsNativeError::error().with_message("scratchpad mutex poisoned"))?
          .insert(key, value);

      Ok(JsValue::Null)
  }

  fn recall_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
      use boa_engine::JsArgs;

      let key = args
          .get_or_undefined(0)
          .to_string(ctx)?
          .to_std_string_escaped();

      let scratchpad = SANDBOX_CTX.with(|cell| {
          let borrow = cell.borrow();
          let sandbox = borrow.as_ref().ok_or_else(|| {
              JsNativeError::error().with_message("sandbox context not initialized")
          })?;
          Ok::<Scratchpad, JsError>(Arc::clone(&sandbox.scratchpad))
      })?;

      let value = scratchpad
          .lock()
          .map_err(|_| JsNativeError::error().with_message("scratchpad mutex poisoned"))?
          .get(&key)
          .cloned();

      match value {
          Some(v) => serde_to_jsvalue(&v, ctx).or(Ok(JsValue::Null)),
          None => Ok(JsValue::Null),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use boa_engine::{Context, Source};

      #[test]
      fn test_registers_remember_and_recall() {
          let mut ctx = Context::default();
          register(&mut ctx).expect("register should not fail");
          for name in ["remember", "recall"] {
              let result = ctx
                  .eval(Source::from_bytes(format!("typeof {name}").as_bytes()))
                  .unwrap();
              assert_eq!(
                  result.as_string().unwrap().to_std_string_escaped(),
                  "function",
                  "{name} should be a function"
              );
          }
      }

      #[test]
      fn test_new_scratchpad_is_empty() {
          let sp = new_scratchpad();
          assert!(sp.lock().unwrap().is_empty());
      }
  }
  ```

- [ ] **Step 4: Run tests**

  ```bash
  cargo test -q -p super-ragondin-codemode 2>&1 | tail -20
  ```

  Expected: `test_registers_remember_and_recall` and `test_new_scratchpad_is_empty` pass. Other tests may fail if `sandbox.rs` imports `scratchpad` before it's wired — that is addressed in Task 2.

- [ ] **Step 5: Format and lint**

  ```bash
  cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
  ```

  Expected: no errors.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/codemode/src/tools/scratchpad.rs crates/codemode/src/tools/mod.rs
  git commit -m "feat(codemode): add scratchpad type and register remember/recall globals"
  ```

---

### Task 2: Wire scratchpad into `SandboxContext` and `Sandbox` (failing test first)

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Write the failing cross-call persistence tests**

  In `sandbox.rs`, inside the existing `#[cfg(test)] mod tests` block, add these tests **before** implementing the changes (they will fail to compile because `Sandbox::new` still takes 3 args):

  ```rust
  #[tokio::test(flavor = "multi_thread")]
  async fn test_scratchpad_persists_across_execute_calls() {
      use crate::tools::scratchpad::new_scratchpad;
      // Use two independent temp dirs to avoid opening two RagStore handles
      // on the same LanceDB path simultaneously.
      let db_a = tempdir().unwrap();
      let db_b = tempdir().unwrap();
      let sync_dir = tempdir().unwrap();
      let store_a = Arc::new(RagStore::open(db_a.path()).await.unwrap());
      let store_b = Arc::new(RagStore::open(db_b.path()).await.unwrap());
      let config_a = RagConfig::from_env_with_db_path(db_a.path().to_path_buf());
      let config_b = RagConfig::from_env_with_db_path(db_b.path().to_path_buf());
      let scratchpad = new_scratchpad();
      let sandbox_a = Sandbox::new(
          store_a,
          config_a,
          sync_dir.path().to_path_buf(),
          Arc::clone(&scratchpad),
      );
      let sandbox_b = Sandbox::new(
          store_b,
          config_b,
          sync_dir.path().to_path_buf(),
          Arc::clone(&scratchpad),
      );
      sandbox_a.execute(r#"remember("x", 42)"#).unwrap();
      let result = sandbox_b.execute(r#"recall("x")"#).unwrap();
      assert_eq!(result, "42", "recall should return the value stored by remember");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn test_scratchpad_recall_missing_key_returns_null() {
      let (sandbox, _db, _sync) = make_sandbox().await;
      let result = sandbox.execute(r#"recall("nonexistent")"#).unwrap();
      assert_eq!(result, "null");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn test_scratchpad_overwrite() {
      // make_sandbox() creates one scratchpad, so all three execute() calls share it.
      let (sandbox, _db, _sync) = make_sandbox().await;
      sandbox.execute(r#"remember("k", "first")"#).unwrap();
      sandbox.execute(r#"remember("k", "second")"#).unwrap();
      let result = sandbox.execute(r#"recall("k")"#).unwrap();
      assert_eq!(result, r#""second""#);
  }
  ```

- [ ] **Step 2: Verify it fails to compile**

  ```bash
  cargo test -q -p super-ragondin-codemode 2>&1 | head -30
  ```

  Expected: compile errors about `Sandbox::new` argument count and missing `scratchpad` field.

- [ ] **Step 3: Update `SandboxContext` to add the `scratchpad` field**

  In `sandbox.rs`, find the `SandboxContext` struct and add the field:

  ```rust
  // Before (existing struct):
  pub struct SandboxContext {
      pub store: Arc<RagStore>,
      pub embedder: Arc<OpenRouterEmbedder>,
      pub config: RagConfig,
      pub handle: tokio::runtime::Handle,
      pub sync_dir: std::path::PathBuf,
  }

  // After:
  pub struct SandboxContext {
      pub store: Arc<RagStore>,
      pub embedder: Arc<OpenRouterEmbedder>,
      pub config: RagConfig,
      pub handle: tokio::runtime::Handle,
      pub sync_dir: std::path::PathBuf,
      pub scratchpad: crate::tools::scratchpad::Scratchpad,
  }
  ```

- [ ] **Step 4: Update the `Sandbox` struct and `Sandbox::new()`**

  In `sandbox.rs`, update the struct and constructor. Remove `const` from `new` since `Arc` is not const-constructible:

  ```rust
  // Before:
  pub struct Sandbox {
      store: Arc<RagStore>,
      config: RagConfig,
      sync_dir: std::path::PathBuf,
  }

  // After:
  pub struct Sandbox {
      store: Arc<RagStore>,
      config: RagConfig,
      sync_dir: std::path::PathBuf,
      scratchpad: crate::tools::scratchpad::Scratchpad,
  }
  ```

  And update `new()`:

  ```rust
  // Before:
  pub const fn new(
      store: Arc<RagStore>,
      config: RagConfig,
      sync_dir: std::path::PathBuf,
  ) -> Self {
      Self { store, config, sync_dir }
  }

  // After:
  #[must_use]
  pub fn new(
      store: Arc<RagStore>,
      config: RagConfig,
      sync_dir: std::path::PathBuf,
      scratchpad: crate::tools::scratchpad::Scratchpad,
  ) -> Self {
      Self { store, config, sync_dir, scratchpad }
  }
  ```

- [ ] **Step 5: Update `Sandbox::execute()` to populate `scratchpad` in the thread-local**

  In `sandbox.rs`, find the `SANDBOX_CTX.with` block inside `execute()` and add the `scratchpad` field:

  ```rust
  SANDBOX_CTX.with(|cell| {
      *cell.borrow_mut() = Some(SandboxContext {
          store: Arc::clone(&self.store),
          embedder,
          config: self.config.clone(),
          handle,
          sync_dir: self.sync_dir.clone(),
          scratchpad: Arc::clone(&self.scratchpad),  // ADD THIS LINE
      });
  });
  ```

- [ ] **Step 6: Register `scratchpad` in `run_boa()`**

  In `sandbox.rs`, find `run_boa` and add the registration after the other tools:

  ```rust
  tools::generate_image::register(&mut ctx)
      .map_err(|e| format!("JS error: register generateImage: {e}"))?;
  tools::scratchpad::register(&mut ctx)
      .map_err(|e| format!("JS error: register scratchpad: {e}"))?;
  ```

- [ ] **Step 7: Update `make_sandbox()` helper in tests**

  In the `#[cfg(test)]` block in `sandbox.rs`, `make_sandbox()` calls `Sandbox::new(store, config, sync_dir)`. Update it to pass a fresh scratchpad:

  ```rust
  async fn make_sandbox() -> (Sandbox, tempfile::TempDir, tempfile::TempDir) {
      use crate::tools::scratchpad::new_scratchpad;
      let db_dir = tempdir().expect("failed to create temp db dir");
      let sync_dir = tempdir().expect("failed to create temp sync dir");
      let store = Arc::new(
          RagStore::open(db_dir.path())
              .await
              .expect("failed to open RagStore"),
      );
      let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
      let sandbox = Sandbox::new(store, config, sync_dir.path().to_path_buf(), new_scratchpad());
      (sandbox, db_dir, sync_dir)
  }
  ```

- [ ] **Step 8: Update `test_sandbox_globals_registered` to include `remember` and `recall`**

  Find `test_sandbox_globals_registered` and update the list:

  ```rust
  for fn_name in &[
      "search",
      "listFiles",
      "getDocument",
      "subAgent",
      "saveFile",
      "listDirs",
      "generateImage",
      "remember",
      "recall",
  ] {
  ```

- [ ] **Step 9: Run all codemode tests**

  ```bash
  cargo test -q -p super-ragondin-codemode 2>&1 | tail -20
  ```

  Expected: all tests pass, including the three new scratchpad tests.

- [ ] **Step 10: Format and lint**

  ```bash
  cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
  ```

  Expected: no errors.

- [ ] **Step 11: Commit**

  ```bash
  git add crates/codemode/src/sandbox.rs
  git commit -m "feat(codemode): wire Scratchpad into SandboxContext and Sandbox"
  ```

---

### Task 3: Thread scratchpad through `engine.rs`

**Files:**
- Modify: `crates/codemode/src/engine.rs`

At this point `engine.rs` won't compile because `Sandbox::new` now requires a 4th argument.

- [ ] **Step 1: Add import and create scratchpad in `ask()`**

  At the top of `engine.rs`, the `use` block already imports what's needed. Add to the body of `ask()`, right after the `client` is built:

  ```rust
  // Existing:
  let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(120))
      .build()
      .context("Failed to build HTTP client")?;

  // Add after:
  let scratchpad = crate::tools::scratchpad::new_scratchpad();
  ```

- [ ] **Step 2: Clone scratchpad inside the tool-call loop**

  Find the `for tool_call in &tool_calls` loop. Add the scratchpad clone **inside** the loop (before the `spawn_blocking` call), then pass it to `Sandbox::new`:

  ```rust
  // Before:
  for tool_call in &tool_calls {
      let store_clone = Arc::clone(&self.store);
      let config_clone = self.config.clone();
      let sync_dir_clone = self.sync_dir.clone();
      let code_clone = tool_call.code.clone();
      let id_clone = tool_call.id.clone();
      handles.push(tokio::task::spawn_blocking(move || {
          let sandbox = Sandbox::new(store_clone, config_clone, sync_dir_clone);
          (id_clone, sandbox.execute(&code_clone))
      }));
  }

  // After:
  for tool_call in &tool_calls {
      let store_clone = Arc::clone(&self.store);
      let config_clone = self.config.clone();
      let sync_dir_clone = self.sync_dir.clone();
      let scratchpad_clone = Arc::clone(&scratchpad);
      let code_clone = tool_call.code.clone();
      let id_clone = tool_call.id.clone();
      handles.push(tokio::task::spawn_blocking(move || {
          let sandbox = Sandbox::new(store_clone, config_clone, sync_dir_clone, scratchpad_clone);
          (id_clone, sandbox.execute(&code_clone))
      }));
  }
  ```

- [ ] **Step 3: Update the `execute_js` tool description string**

  Find `execute_js_tool_definition()` in `engine.rs`. Update the `"description"` field:

  ```rust
  // Before:
  "description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), listDirs(), and generateImage() functions to query the document database, write files, and generate images.",

  // After:
  "description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), listDirs(), generateImage(), remember(), and recall() functions to query the document database, write files, generate images, and store values across tool calls.",
  ```

- [ ] **Step 4: Build to verify no compile errors**

  ```bash
  cargo build -p super-ragondin-codemode 2>&1 | grep -E "^error"
  ```

  Expected: no errors.

- [ ] **Step 5: Run all tests**

  ```bash
  cargo test -q -p super-ragondin-codemode 2>&1 | tail -20
  ```

  Expected: all tests pass.

- [ ] **Step 6: Format and lint**

  ```bash
  cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error"
  ```

  Expected: no errors.

- [ ] **Step 7: Commit**

  ```bash
  git add crates/codemode/src/engine.rs
  git commit -m "feat(codemode): thread Scratchpad through engine ask() loop"
  ```

---

### Task 4: Document `remember`/`recall` in the system prompt

**Files:**
- Modify: `crates/codemode/src/prompt.rs`

- [ ] **Step 1: Update the system prompt**

  In `prompt.rs`, find the `Available JavaScript functions:` section. Add after the `generateImage` entry (and before `Rules:`):

  ```
    remember(key, value)
      Store a JSON-serializable value under a string key for this session.
      Only JSON-serializable values are stored (objects, arrays, strings, numbers, booleans).
      Returns: null

    recall(key)
      Retrieve a value previously stored with remember().
      Returns: the stored value, or null if the key was not set.
  ```

  Also update the `Rules:` section — find:
  ```
  - Each execute_js call is a fresh context — variables do not persist between calls
  ```
  and change it to:
  ```
  - Each execute_js call is a fresh context — JS variables do not persist between calls
  - Use remember(key, value) / recall(key) to store values across execute_js calls
  - Do not write to the same key from two concurrent tool calls in the same iteration — order is non-deterministic
  ```

  Also add an example near the bottom:
  ```js
  // Store an intermediate result and reuse it in a later call
  const files = listFiles({ sort: "recent", limit: 5 });
  remember("recent_ids", files.map(f => f.doc_id));
  ```

- [ ] **Step 2: Update the `test_prompt_contains_key_elements` test**

  In `prompt.rs`, add two assertions to the existing test:

  ```rust
  assert!(p.contains("remember("));
  assert!(p.contains("recall("));
  ```

- [ ] **Step 3: Run the prompt test**

  ```bash
  cargo test -q -p super-ragondin-codemode test_prompt_contains_key_elements 2>&1
  ```

  Expected: PASS.

- [ ] **Step 4: Run the full test suite one final time**

  ```bash
  cargo test -q 2>&1 | tail -20
  ```

  Expected: all tests pass across the whole workspace.

- [ ] **Step 5: Format and lint**

  ```bash
  cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error"
  ```

  Expected: no errors.

- [ ] **Step 6: Final commit**

  ```bash
  git add crates/codemode/src/prompt.rs
  git commit -m "docs(codemode): add remember/recall to system prompt and rules"
  ```
