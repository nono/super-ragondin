# ask-user clarification tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `askUser(question, choices)` JS global that pauses the codemode loop and prompts the user for clarification, with CLI (stdin/stdout) and GUI (Tauri event) backends.

**Architecture:** A `UserInteraction` trait in `crates/codemode` is stored optionally in `Sandbox` and `SandboxContext`. The `askUser` JS global reads it from `SANDBOX_CTX` (same pattern as `sendMail` reads `cozy_client`). It is only registered when the backend is present; the system prompt only mentions it when interactive.

**Tech Stack:** Rust, Boa JS engine, Tauri v2, Svelte 5, tauri-specta

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `crates/codemode/src/interaction.rs` | `UserInteraction` trait |
| Modify | `crates/codemode/src/lib.rs` | expose `interaction` module |
| Create | `crates/codemode/src/tools/ask_user.rs` | `register()` JS global + `resolve_answer()` helper |
| Modify | `crates/codemode/src/tools/mod.rs` | add `ask_user` module |
| Modify | `crates/codemode/src/sandbox.rs` | add `interaction` to `SandboxContext` and `Sandbox`; conditional registration in `run_boa` |
| Modify | `crates/codemode/src/prompt.rs` | `system_prompt(interactive: bool) -> String` |
| Modify | `crates/codemode/src/engine.rs` | add `interaction` field; thread through to `Sandbox` |
| Modify | `crates/cli/src/main.rs` | `CliInteraction` + pass to engine |
| Modify | `crates/gui/src/commands.rs` | `AskUserEvent`, `AskUserState`, `answer_user`, `GuiInteraction`; wire into `ask_question` |
| Modify | `crates/gui/src/main.rs` | `.manage(AskUserState::default())` |
| Modify | `gui-frontend/src/bindings.ts` | add `answerUser` command + `askUserEvent` event |
| Modify | `gui-frontend/src/lib/AskPanel.svelte` | `'clarifying'` state: show choices, call `answerUser` |

---

### Task 1: `UserInteraction` trait

**Files:**
- Create: `crates/codemode/src/interaction.rs`
- Modify: `crates/codemode/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to bottom of `crates/codemode/src/lib.rs` (after existing `pub mod`s):

```rust
#[cfg(test)]
mod interaction_trait_test {
    use std::sync::Arc;
    use crate::interaction::UserInteraction;

    struct Echo;
    impl UserInteraction for Echo {
        fn ask(&self, _question: &str, _choices: &[&str]) -> String {
            "echo".to_string()
        }
    }

    #[test]
    fn test_trait_object_works() {
        let i: Arc<dyn UserInteraction> = Arc::new(Echo);
        assert_eq!(i.ask("q?", &["a", "b"]), "echo");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /home/nono/dev/super-ragondin/tools
cargo test -q -p super-ragondin-codemode interaction_trait_test 2>&1 | tail -5
```

Expected: compile error — `crate::interaction` not found.

- [ ] **Step 3: Create `interaction.rs`**

```rust
/// Backend for user interaction during codemode execution.
///
/// Implement this trait to provide a concrete I/O mechanism (CLI stdin,
/// Tauri event, etc.). The trait is `Send + Sync` so it can be stored
/// inside `Arc` and shared across threads.
pub trait UserInteraction: Send + Sync {
    /// Ask the user a clarifying question with 2–3 labelled choices.
    ///
    /// The user may pick a numbered choice (1-based) or type a free-form
    /// answer. Returns the user's response as a plain string.
    fn ask(&self, question: &str, choices: &[&str]) -> String;
}
```

- [ ] **Step 4: Expose the module in `lib.rs`**

Add after the existing `pub mod` lines in `crates/codemode/src/lib.rs`:

```rust
pub mod interaction;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -q -p super-ragondin-codemode interaction_trait_test 2>&1 | tail -5
```

Expected: `test interaction_trait_test::test_trait_object_works ... ok`

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/codemode/src/interaction.rs crates/codemode/src/lib.rs
git commit -m "feat(codemode): add UserInteraction trait"
```

---

### Task 2: `ask_user.rs` tool — pure logic

**Files:**
- Create: `crates/codemode/src/tools/ask_user.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/codemode/src/tools/ask_user.rs` with tests only:

```rust
use boa_engine::{Context, JsError};

/// Resolve a raw user input string against a choices list.
///
/// If `input` is a decimal integer in range 1..=choices.len(), returns the
/// corresponding choice text. Otherwise returns `input` verbatim (trimmed).
pub fn resolve_answer(input: &str, choices: &[&str]) -> String {
    todo!()
}

/// Register the `askUser(question, choices)` global function.
///
/// Only call this when a `UserInteraction` backend is present in `SANDBOX_CTX`.
///
/// # Errors
/// Returns error if the global function cannot be registered.
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_number_in_range() {
        assert_eq!(resolve_answer("2", &["alpha", "beta", "gamma"]), "beta");
    }

    #[test]
    fn test_resolve_first_choice() {
        assert_eq!(resolve_answer("1", &["yes", "no"]), "yes");
    }

    #[test]
    fn test_resolve_last_choice() {
        assert_eq!(resolve_answer("3", &["x", "y", "z"]), "z");
    }

    #[test]
    fn test_resolve_out_of_range_returns_verbatim() {
        assert_eq!(resolve_answer("0", &["a", "b"]), "0");
        assert_eq!(resolve_answer("5", &["a", "b"]), "5");
    }

    #[test]
    fn test_resolve_freeform_returns_trimmed() {
        assert_eq!(resolve_answer("  my answer  ", &["a", "b"]), "my answer");
    }

    #[test]
    fn test_resolve_nonnumeric_returns_verbatim() {
        assert_eq!(resolve_answer("custom", &["a", "b"]), "custom");
    }
}
```

- [ ] **Step 2: Add module to `mod.rs`**

Add to `crates/codemode/src/tools/mod.rs`:

```rust
pub mod ask_user;
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode ask_user::tests 2>&1 | tail -8
```

Expected: `panicked at 'not yet implemented'`

- [ ] **Step 4: Implement `resolve_answer`**

Replace the `todo!()` in `resolve_answer`:

```rust
pub fn resolve_answer(input: &str, choices: &[&str]) -> String {
    let trimmed = input.trim();
    if let Ok(n) = trimmed.parse::<usize>() {
        if n >= 1 && n <= choices.len() {
            return choices[n - 1].to_string();
        }
    }
    trimmed.to_string()
}
```

- [ ] **Step 5: Implement `register`**

Replace the `todo!()` in `register`, adding the required imports at the top of the file:

```rust
use std::sync::Arc;
use boa_engine::{Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use crate::sandbox::SANDBOX_CTX;

pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("askUser"),
        2,
        NativeFunction::from_fn_ptr(ask_user_fn),
    )?;
    Ok(())
}

fn ask_user_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use crate::sandbox::jsvalue_to_serde;

    let question = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_lossy();

    let choices_js = args.get_or_undefined(1);
    let choices_serde = jsvalue_to_serde(choices_js.clone(), ctx);
    let choices_vec: Vec<String> = match &choices_serde {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => {
            return Err(JsNativeError::typ()
                .with_message("askUser: choices must be an array")
                .into());
        }
    };

    if choices_vec.len() < 2 || choices_vec.len() > 3 {
        return Err(JsNativeError::range()
            .with_message(format!(
                "askUser: choices must have 2 or 3 entries, got {}",
                choices_vec.len()
            ))
            .into());
    }

    let interaction = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        sandbox
            .interaction
            .clone()
            .ok_or_else(|| JsNativeError::error().with_message("askUser not available"))
    })?;

    let choices_refs: Vec<&str> = choices_vec.iter().map(String::as_str).collect();
    let answer = interaction.ask(&question, &choices_refs);

    Ok(JsValue::from(js_string!(answer)))
}
```

- [ ] **Step 6: Run unit tests to verify they pass**

```bash
cargo test -q -p super-ragondin-codemode ask_user::tests 2>&1 | tail -8
```

Expected: all 6 tests pass.

- [ ] **Step 7: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add crates/codemode/src/tools/ask_user.rs crates/codemode/src/tools/mod.rs
git commit -m "feat(codemode): add ask_user tool with resolve_answer helper"
```

---

### Task 3: Thread `interaction` through `SandboxContext` and `Sandbox`

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`

- [ ] **Step 1: Write the failing sandbox tests**

Add to the `tests` module in `crates/codemode/src/sandbox.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_ask_user_not_registered_without_interaction() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute("typeof askUser").unwrap();
    assert_eq!(result, "\"undefined\"", "askUser must not exist when no interaction provided");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ask_user_registered_with_interaction() {
    use crate::tools::scratchpad::new_scratchpad;
    use crate::interaction::UserInteraction;

    struct Always(String);
    impl UserInteraction for Always {
        fn ask(&self, _q: &str, _c: &[&str]) -> String { self.0.clone() }
    }

    let db_dir = tempdir().expect("db_dir");
    let sync_dir = tempdir().expect("sync_dir");
    let store = Arc::new(RagStore::open(db_dir.path()).await.expect("store"));
    let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    let interaction: Arc<dyn crate::interaction::UserInteraction> =
        Arc::new(Always("choice B".to_string()));
    let sandbox = Sandbox::new(
        store,
        config,
        sync_dir.path().to_path_buf(),
        new_scratchpad(),
        None,
        Some(interaction),
    );
    let result = sandbox.execute("typeof askUser").unwrap();
    assert_eq!(result, "\"function\"", "askUser must be a function when interaction is provided");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ask_user_returns_interaction_answer() {
    use crate::tools::scratchpad::new_scratchpad;
    use crate::interaction::UserInteraction;

    struct Always(String);
    impl UserInteraction for Always {
        fn ask(&self, _q: &str, _c: &[&str]) -> String { self.0.clone() }
    }

    let db_dir = tempdir().expect("db_dir");
    let sync_dir = tempdir().expect("sync_dir");
    let store = Arc::new(RagStore::open(db_dir.path()).await.expect("store"));
    let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    let interaction: Arc<dyn crate::interaction::UserInteraction> =
        Arc::new(Always("option two".to_string()));
    let sandbox = Sandbox::new(
        store,
        config,
        sync_dir.path().to_path_buf(),
        new_scratchpad(),
        None,
        Some(interaction),
    );
    let result = sandbox
        .execute(r#"askUser("Which style?", ["option one", "option two", "option three"])"#)
        .unwrap();
    // result is a JSON string (quoted)
    let answer: String = serde_json::from_str(&result).unwrap();
    assert_eq!(answer, "option two");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode test_ask_user 2>&1 | tail -8
```

Expected: compile errors — `Sandbox::new` doesn't accept 6 args yet.

- [ ] **Step 3: Add `interaction` to `SandboxContext`**

In `sandbox.rs`, add the import at the top:

```rust
use crate::interaction::UserInteraction;
```

Add `interaction` field to `SandboxContext`:

```rust
pub struct SandboxContext {
    pub store: Arc<RagStore>,
    pub embedder: Arc<OpenRouterEmbedder>,
    pub config: RagConfig,
    pub handle: tokio::runtime::Handle,
    pub sync_dir: std::path::PathBuf,
    pub scratchpad: crate::tools::scratchpad::Scratchpad,
    pub cozy_client: Option<Arc<CozyClient>>,
    pub interaction: Option<Arc<dyn UserInteraction>>,
}
```

- [ ] **Step 4: Add `interaction` field to `Sandbox`**

Replace the `Sandbox` struct and `new` method (remove `const fn` since `Arc<dyn Trait>` is not const):

```rust
#[allow(dead_code)]
pub struct Sandbox {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
    scratchpad: crate::tools::scratchpad::Scratchpad,
    cozy_client: Option<Arc<CozyClient>>,
    interaction: Option<Arc<dyn UserInteraction>>,
}

#[allow(dead_code)]
impl Sandbox {
    #[must_use]
    pub fn new(
        store: Arc<RagStore>,
        config: RagConfig,
        sync_dir: std::path::PathBuf,
        scratchpad: crate::tools::scratchpad::Scratchpad,
        cozy_client: Option<Arc<CozyClient>>,
        interaction: Option<Arc<dyn UserInteraction>>,
    ) -> Self {
        Self {
            store,
            config,
            sync_dir,
            scratchpad,
            cozy_client,
            interaction,
        }
    }
```

- [ ] **Step 5: Update `execute()` to populate `interaction` in `SandboxContext`**

In `execute()`, add `interaction` to the `SandboxContext` construction:

```rust
SANDBOX_CTX.with(|cell| {
    *cell.borrow_mut() = Some(SandboxContext {
        store: Arc::clone(&self.store),
        embedder,
        config: self.config.clone(),
        handle,
        sync_dir: self.sync_dir.clone(),
        scratchpad: Arc::clone(&self.scratchpad),
        cozy_client: self.cozy_client.clone(),
        interaction: self.interaction.clone(),
    });
});
```

- [ ] **Step 6: Update `run_boa()` to conditionally register `askUser`**

In `run_boa()`, remove `#[allow(clippy::unused_self)]` and add after the other `register` calls:

```rust
if self.interaction.is_some() {
    tools::ask_user::register(&mut ctx)
        .map_err(|e| format!("JS error: register askUser: {e}"))?;
}
```

- [ ] **Step 7: Fix the `make_sandbox()` helper in tests to pass `None` for interaction**

Update the `make_sandbox()` helper inside the `tests` module:

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
    let sandbox = Sandbox::new(
        store,
        config,
        sync_dir.path().to_path_buf(),
        new_scratchpad(),
        None,
        None, // no interaction in these tests
    );
    (sandbox, db_dir, sync_dir)
}
```

Also update the `test_scratchpad_persists_across_execute_calls` test — both `Sandbox::new` calls need a 6th `None` arg:

```rust
let sandbox_a = Sandbox::new(store_a, config_a, sync_dir.path().to_path_buf(), Arc::clone(&scratchpad), None, None);
let sandbox_b = Sandbox::new(store_b, config_b, sync_dir.path().to_path_buf(), Arc::clone(&scratchpad), None, None);
```

- [ ] **Step 8: Update `test_sandbox_globals_registered` — `askUser` must not appear in the list**

The existing test already won't have `askUser` since we pass `None`. Verify the list in that test does **not** include `askUser`:

```rust
for fn_name in &[
    "search",
    "listFiles",
    "getDocument",
    "subAgent",
    "saveFile",
    "mkdir",
    "listDirs",
    "generateImage",
    "remember",
    "recall",
    "sendMail",
    // askUser is NOT listed — it's absent when interaction is None
] {
```

(No change needed if `askUser` is not in the list — just verify it isn't.)

- [ ] **Step 9: Run all sandbox tests**

```bash
cargo test -q -p super-ragondin-codemode 2>&1 | tail -10
```

Expected: all tests pass including the 3 new `test_ask_user_*` tests.

- [ ] **Step 10: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 11: Commit**

```bash
git add crates/codemode/src/sandbox.rs crates/codemode/src/tools/ask_user.rs
git commit -m "feat(codemode): thread interaction through SandboxContext and Sandbox"
```

---

### Task 4: Update `system_prompt` to accept `interactive: bool`

**Files:**
- Modify: `crates/codemode/src/prompt.rs`
- Modify: `crates/codemode/src/engine.rs` (just the call site — full engine update is Task 5)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `prompt.rs`:

```rust
#[test]
fn test_prompt_interactive_contains_ask_user() {
    let p = system_prompt(true);
    assert!(p.contains("askUser("), "interactive prompt must mention askUser");
}

#[test]
fn test_prompt_non_interactive_omits_ask_user() {
    let p = system_prompt(false);
    assert!(!p.contains("askUser("), "non-interactive prompt must not mention askUser");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin-codemode prompt:: 2>&1 | tail -8
```

Expected: compile error — `system_prompt` takes 0 arguments.

- [ ] **Step 3: Change `system_prompt` signature and body**

Replace the entire `system_prompt` function with a regular (non-const) function that builds a `String`:

```rust
/// System prompt for the code-mode LLM agent.
///
/// Pass `interactive = true` when a `UserInteraction` backend is available,
/// which adds the `askUser()` tool description to the prompt.
#[must_use]
pub fn system_prompt(interactive: bool) -> String {
    let base = r##"You are Super Ragondin, a helpful assistant with access to a personal document database.
To answer questions, use the `execute_js` tool to query the database before responding.

Available JavaScript functions:

  search(query, options?)
    Semantic vector search. Options: { limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, chunk_text, mime_type, mtime }, ...]
    mtime is an ISO 8601 string (e.g. "2024-06-15T10:30:00Z")

  listFiles(options?)
    Discover files by metadata. Options: { sort: "recent"|"oldest", limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, mime_type, mtime }, ...]

  getDocument(docId)
    Fetch all chunks of a document in order.
    Returns: [{ chunk_index, chunk_text }, ...]

  subAgent(systemPrompt, userPrompt)
    Ask a fast LLM to process text (summarize, extract, etc.)
    Returns: string

  saveFile(path, content, options?)
    Write a file into the sync directory. options: { encoding: "utf8" | "base64" }
    Default encoding is "utf8". Use "base64" for binary content.
    Creates intermediate directories automatically.
    Returns: null

  mkdir(path)
    Create a directory (and any intermediate directories) in the sync directory.
    Returns: null

  listDirs(prefix?)
    Non-recursive: list only immediate subdirectory names at a given path within the sync directory.
    Returns: string[] — directory names only, sorted alphabetically

  generateImage(prompt, options?)
    Generate an image via OpenRouter and return it as a base64 string.
    Options: { path, aspect, size, reference }
      path: relative path in sync_dir to save the image (e.g. "images/out.png")
      aspect: aspect ratio string, e.g. "1:1", "16:9", "4:3" (default: "1:1")
      size: "0.5K" | "1K" | "2K" | "4K" (default: "0.5K")
      reference: relative path to an existing image for image-to-image generation
    Returns: base64-encoded image string (without the data: prefix)
    Side effect: if path is given, the image is written to sync_dir/path

  remember(key, value)
    Store a JSON-serializable value under a string key for this session.
    Only JSON-serializable values are stored (objects, arrays, strings, numbers, booleans).
    Returns: null

  recall(key)
    Retrieve a value previously stored with remember().
    Returns: the stored value, or null if the key was not set."##;

    let interactive_section = r#"

  askUser(question, choices)
    Ask the user a clarifying question with 2–3 labelled choices.
    choices must be an array of 2 or 3 strings.
    The user may pick a numbered option or type a free-form answer.
    Returns: string — the user's answer
    Use sparingly — only when you genuinely cannot proceed without clarification."#;

    let rules = r##"

Rules:
- Each execute_js call is a fresh context — JS variables do not persist between calls
- Use remember(key, value) / recall(key) to store values across execute_js calls
- Do not write to the same key from two concurrent tool calls in the same iteration — order is non-deterministic
- The last expression in your JS code is the return value (JSON-serialized)
- Dates in mtime, after, before are ISO 8601 strings
- Use multiple execute_js calls when gathering information in stages
- For complex questions, decompose: search each aspect separately, use subAgent() to summarize each, then synthesize a final answer
- When the user refers to a recent or specific document, start with listFiles({ sort: "recent" })
- Once you have enough information, write your final answer directly without another tool call

Examples:

// Simple search
search("project deadline", { limit: 5 })

// Get the most recently added document
const files = listFiles({ sort: "recent", limit: 1 });
getDocument(files[0].doc_id)

// Multi-aspect question with sub-agent summarization
const budgetChunks = search("budget forecasts", { limit: 3 });
const headcountChunks = search("team headcount", { limit: 3 });
const budgetSummary = subAgent("Summarize concisely.", budgetChunks.map(r => r.chunk_text).join("\n"));
const headcountSummary = subAgent("Summarize concisely.", headcountChunks.map(r => r.chunk_text).join("\n"));
({ budget: budgetSummary, headcount: headcountSummary })

// Search only in a specific folder and date range
search("meeting notes", { pathPrefix: "work/", after: "2025-01-01", limit: 10 })

// Discover top-level directories
listDirs()

// Explore a subdirectory before saving
const dirs = listDirs("work");
// dirs might be ["meetings", "projects"]

// Create a folder to organize output
mkdir("linkedin-2026-03-27")
saveFile("linkedin-2026-03-27/draft1.md", "# Draft 1\n\n...")

// Save a text summary
saveFile("notes/summary.md", "# Summary\n\nKey points...", { encoding: "utf8" })

// Save a generated image (base64)
saveFile("images/chart.png", base64EncodedPngString, { encoding: "base64" })

// Generate a watercolor-style mindmap and save it
const b64 = generateImage(
  "Watercolor mindmap: key topics from the meeting notes",
  { path: "images/mindmap.png", aspect: "4:3", size: "1K" }
)

// Store an intermediate result and reuse it in a later call
const files = listFiles({ sort: "recent", limit: 5 });
remember("recent_ids", files.map(f => f.doc_id));"##;

    if interactive {
        format!("{base}{interactive_section}{rules}")
    } else {
        format!("{base}{rules}")
    }
}
```

- [ ] **Step 4: Fix the existing test — pass `false`**

In the existing `test_prompt_contains_key_elements` test, change the call:

```rust
let p = system_prompt(false);
```

- [ ] **Step 5: Run all prompt tests**

```bash
cargo test -q -p super-ragondin-codemode prompt:: 2>&1 | tail -8
```

Expected: all tests pass including the 2 new ones.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors. (There will be a compile error in `engine.rs` where `system_prompt()` is called with no args — fix it by temporarily passing `false`: `system_prompt(false)`. The full engine update is Task 5.)

- [ ] **Step 7: Temporary fix in `engine.rs`**

Find line 119 in `engine.rs`:

```rust
let mut messages = vec![serde_json::json!({"role": "system", "content": system_prompt()})];
```

Change to:

```rust
let mut messages = vec![serde_json::json!({"role": "system", "content": system_prompt(false)})];
```

- [ ] **Step 8: Verify the codemode crate builds**

```bash
cargo build -q -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 9: Commit**

```bash
git add crates/codemode/src/prompt.rs crates/codemode/src/engine.rs
git commit -m "feat(codemode): make system_prompt interactive-aware"
```

---

### Task 5: Add `interaction` to `CodeModeEngine`

**Files:**
- Modify: `crates/codemode/src/engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `engine.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_engine_passes_interaction_to_sandbox() {
    use crate::interaction::UserInteraction;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    struct FlagInteraction(Arc<AtomicBool>);
    impl UserInteraction for FlagInteraction {
        fn ask(&self, _q: &str, _c: &[&str]) -> String {
            self.0.store(true, Ordering::SeqCst);
            "yes".to_string()
        }
    }

    let db_dir = tempfile::tempdir().expect("db_dir");
    let sync_dir = tempfile::tempdir().expect("sync_dir");
    let store = super_ragondin_rag::store::RagStore::open(db_dir.path()).await.expect("store");
    let config = super_ragondin_rag::config::RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
    let flag = Arc::new(AtomicBool::new(false));
    let interaction: Arc<dyn UserInteraction> = Arc::new(FlagInteraction(Arc::clone(&flag)));

    let engine = CodeModeEngine {
        store: Arc::new(store),
        config,
        sync_dir: sync_dir.path().to_path_buf(),
        cozy_client: None,
        interaction: Some(interaction),
    };

    // Build a sandbox directly and verify askUser is registered
    use crate::tools::scratchpad::new_scratchpad;
    let sandbox = crate::sandbox::Sandbox::new(
        Arc::clone(&engine.store),
        engine.config.clone(),
        engine.sync_dir.clone(),
        new_scratchpad(),
        None,
        engine.interaction.clone(),
    );
    let result = sandbox.execute("typeof askUser").unwrap();
    assert_eq!(result, "\"function\"");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-codemode test_engine_passes_interaction 2>&1 | tail -8
```

Expected: compile error — `CodeModeEngine` struct literal missing `interaction` field.

- [ ] **Step 3: Add `interaction` field to `CodeModeEngine`**

In `engine.rs`, add the import at top:

```rust
use crate::interaction::UserInteraction;
```

Update the `CodeModeEngine` struct:

```rust
pub struct CodeModeEngine {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
    cozy_client: Option<Arc<CozyClient>>,
    interaction: Option<Arc<dyn UserInteraction>>,
}
```

- [ ] **Step 4: Update `CodeModeEngine::new()`**

```rust
pub async fn new(
    config: RagConfig,
    sync_dir: std::path::PathBuf,
    cozy_client: Option<Arc<CozyClient>>,
    interaction: Option<Arc<dyn UserInteraction>>,
) -> Result<Self> {
    let store = Arc::new(RagStore::open(&config.db_path).await?);
    Ok(Self {
        store,
        config,
        sync_dir,
        cozy_client,
        interaction,
    })
}
```

- [ ] **Step 5: Update `ask()` to use `interaction`**

In `ask()`, replace the `system_prompt(false)` temporary fix and the `Sandbox::new` call:

```rust
// system prompt — mention askUser only when interactive
let mut messages = vec![serde_json::json!({"role": "system", "content": system_prompt(self.interaction.is_some())})];
```

In the `spawn_blocking` closure, add `interaction_clone`:

```rust
let interaction_clone = self.interaction.clone();
// ...
handles.push(tokio::task::spawn_blocking(move || {
    let sandbox = Sandbox::new(
        store_clone,
        config_clone,
        sync_dir_clone,
        scratchpad_clone,
        cozy_client_clone,
        interaction_clone,
    );
    (id_clone, sandbox.execute(&code_clone))
}));
```

- [ ] **Step 6: Fix `make_engine_for_ctx_test` helper**

In the `tests` module, update `make_engine_for_ctx_test` to add `interaction: None`:

```rust
let engine = CodeModeEngine {
    store: Arc::new(store),
    config,
    sync_dir: sync_dir.path().to_path_buf(),
    cozy_client: None,
    interaction: None,
};
```

- [ ] **Step 7: Run all codemode tests**

```bash
cargo test -q -p super-ragondin-codemode 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 8: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-codemode 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 9: Commit**

```bash
git add crates/codemode/src/engine.rs
git commit -m "feat(codemode): add interaction field to CodeModeEngine"
```

---

### Task 6: `CliInteraction` in the CLI

**Files:**
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Write failing tests**

Add a test module at the bottom of `crates/cli/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_resolve_number_picks_choice() {
        assert_eq!(resolve_answer("2", &["alpha", "beta", "gamma"]), "beta");
    }

    #[test]
    fn test_cli_resolve_first() {
        assert_eq!(resolve_answer("1", &["yes", "no"]), "yes");
    }

    #[test]
    fn test_cli_resolve_out_of_range_verbatim() {
        assert_eq!(resolve_answer("0", &["a", "b"]), "0");
        assert_eq!(resolve_answer("4", &["a", "b", "c"]), "4");
    }

    #[test]
    fn test_cli_resolve_freeform() {
        assert_eq!(resolve_answer("my custom answer", &["a", "b"]), "my custom answer");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q -p super-ragondin 2>&1 | tail -8
```

Expected: compile error — `resolve_answer` not defined.

- [ ] **Step 3: Add `resolve_answer` and `CliInteraction` before `cmd_ask`**

Insert before the `cmd_ask` function in `main.rs`:

```rust
/// Resolve a raw user input string against a choices list.
///
/// If `input` is a decimal integer in range 1..=choices.len(), returns the
/// corresponding choice text. Otherwise returns `input` verbatim (trimmed).
fn resolve_answer(input: &str, choices: &[&str]) -> String {
    let trimmed = input.trim();
    if let Ok(n) = trimmed.parse::<usize>() {
        if n >= 1 && n <= choices.len() {
            return choices[n - 1].to_string();
        }
    }
    trimmed.to_string()
}

struct CliInteraction;

impl super_ragondin_codemode::interaction::UserInteraction for CliInteraction {
    fn ask(&self, question: &str, choices: &[&str]) -> String {
        use std::io::Write as _;
        println!("\n{question}");
        for (i, c) in choices.iter().enumerate() {
            println!("  {}. {}", i + 1, c);
        }
        print!("\nYour answer (number or free text): ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        resolve_answer(&line, choices)
    }
}
```

- [ ] **Step 4: Update `cmd_ask` to pass `CliInteraction` to the engine**

In `cmd_ask`, find:

```rust
let engine = CodeModeEngine::new(rag_config, config.sync_dir, cozy_client)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
```

Replace with:

```rust
let interaction: Option<std::sync::Arc<dyn super_ragondin_codemode::interaction::UserInteraction>> =
    Some(std::sync::Arc::new(CliInteraction));
let engine = CodeModeEngine::new(rag_config, config.sync_dir, cozy_client, interaction)
    .await
    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -q -p super-ragondin 2>&1 | tail -8
```

Expected: all 4 new tests pass.

- [ ] **Step 6: Verify the CLI crate builds**

```bash
cargo build -q -p super-ragondin 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 7: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): add CliInteraction for askUser clarification"
```

---

### Task 7: GUI — `AskUserEvent`, `AskUserState`, `answer_user`, `GuiInteraction`

**Files:**
- Modify: `crates/gui/src/commands.rs`
- Modify: `crates/gui/src/main.rs`

- [ ] **Step 1: Write a failing test for `answer_user` when no sender is pending**

Add to the `tests` module in `commands.rs` (near the bottom, before `export_bindings`):

```rust
#[test]
fn test_answer_user_no_pending_sender_is_noop() {
    // AskUserState with no sender — answer_user should not panic
    let state = AskUserState::default();
    let mut guard = state.sender.lock().unwrap();
    // No sender present — take returns None, which is fine
    assert!(guard.take().is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -q -p super-ragondin-gui test_answer_user_no_pending_sender 2>&1 | tail -8
```

Expected: compile error — `AskUserState` not defined.

- [ ] **Step 3: Add `AskUserEvent` and `AskUserState`**

Add after the existing event structs (near `AuthCompleteEvent`) in `commands.rs`:

```rust
/// Payload emitted when the agent needs the user to pick a clarification choice.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AskUserEvent {
    pub question: String,
    pub choices: Vec<String>,
}

/// Managed state holding the pending `Sender` for `answer_user`.
#[derive(Default)]
pub struct AskUserState {
    pub sender: std::sync::Mutex<Option<std::sync::mpsc::Sender<String>>>,
}
```

- [ ] **Step 4: Add `answer_user` Tauri command**

Add after `ask_question`:

```rust
/// Deliver the user's answer to a pending `askUser()` call in the codemode sandbox.
///
/// If no `askUser()` call is pending (no sender waiting), this is a no-op.
#[tauri::command]
#[specta::specta]
pub fn answer_user(answer: String, state: tauri::State<AskUserState>) -> Result<(), String> {
    let mut guard = state
        .sender
        .lock()
        .map_err(|e| format!("mutex poisoned: {e}"))?;
    if let Some(tx) = guard.take() {
        tx.send(answer).ok();
    }
    Ok(())
}
```

- [ ] **Step 5: Add `GuiInteraction`**

Add after `answer_user`:

```rust
/// GUI backend for `UserInteraction`: emits a Tauri event and blocks until
/// `answer_user` delivers the response via an `mpsc` channel.
pub struct GuiInteraction {
    pub app_handle: tauri::AppHandle,
}

impl super_ragondin_codemode::interaction::UserInteraction for GuiInteraction {
    fn ask(&self, question: &str, choices: &[&str]) -> String {
        use tauri::Manager as _;
        let (tx, rx) = std::sync::mpsc::channel();
        let state = self.app_handle.state::<AskUserState>();
        {
            let mut guard = state.sender.lock().expect("AskUserState mutex poisoned");
            *guard = Some(tx);
        }
        self.app_handle
            .emit(
                AskUserEvent::NAME,
                AskUserEvent {
                    question: question.to_string(),
                    choices: choices.iter().map(|s| s.to_string()).collect(),
                },
            )
            .ok();
        rx.recv().unwrap_or_default()
    }
}
```

- [ ] **Step 6: Update `ask_question` to use `GuiInteraction`**

Replace the current `ask_question` command body (it used to just delegate to `ask_question_from`):

```rust
#[tauri::command]
#[specta::specta]
pub async fn ask_question(
    question: String,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    use super_ragondin_codemode::engine::CodeModeEngine;
    use super_ragondin_codemode::interaction::UserInteraction;
    use super_ragondin_rag::config::RagConfig;

    let config = Config::load(&config_path())
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

    let interaction: std::sync::Arc<dyn UserInteraction> =
        std::sync::Arc::new(GuiInteraction { app_handle });

    let engine = CodeModeEngine::new(rag_config, config.sync_dir, None, Some(interaction))
        .await
        .map_err(|e| e.to_string())?;

    engine.ask(&question, None).await.map_err(|e| e.to_string())
}
```

- [ ] **Step 7: Update `ask_question_from` to pass `None` for interaction (kept for tests)**

In `ask_question_from`, update the `CodeModeEngine::new` call to add the `interaction` parameter:

```rust
let engine = CodeModeEngine::new(rag_config, config.sync_dir, None, None)
    .await
    .map_err(|e| e.to_string())?;
```

- [ ] **Step 8: Update `make_builder()` to register `answer_user` and `AskUserEvent`**

```rust
pub fn make_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            get_app_state,
            init_config,
            set_api_key,
            start_auth,
            start_sync,
            get_recent_files,
            get_suggestions,
            ask_question,
            answer_user,
        ])
        .events(tauri_specta::collect_events![
            AuthCompleteEvent,
            AuthErrorEvent,
            SyncStatusEvent,
            AskUserEvent,
        ])
}
```

- [ ] **Step 9: Manage `AskUserState` in `main.rs`**

In `crates/gui/src/main.rs`, add `.manage(commands::AskUserState::default())` after `.manage(SyncGuard::default())`:

```rust
tauri::Builder::default()
    .manage(SyncGuard::default())
    .manage(commands::AskUserState::default())
    .invoke_handler(builder.invoke_handler())
```

- [ ] **Step 10: Run all GUI tests**

```bash
cargo test -q -p super-ragondin-gui 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 11: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features -p super-ragondin-gui 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 12: Commit**

```bash
git add crates/gui/src/commands.rs crates/gui/src/main.rs
git commit -m "feat(gui): add GuiInteraction, AskUserEvent, answer_user command"
```

---

### Task 8: Regenerate TypeScript bindings and update the frontend

**Files:**
- Modify: `gui-frontend/src/bindings.ts`
- Modify: `gui-frontend/src/lib/AskPanel.svelte`

- [ ] **Step 1: Regenerate `bindings.ts`**

Run the export test (it is marked `#[ignore]`, so use `--ignored`):

```bash
cargo test -q -p super-ragondin-gui export_bindings -- --ignored 2>&1 | tail -5
```

Expected: `test tests::export_bindings ... ok` and `gui-frontend/src/bindings.ts` updated.

- [ ] **Step 2: Verify bindings contain the new items**

```bash
grep -n "answerUser\|AskUserEvent\|askUserEvent" /home/nono/dev/super-ragondin/tools/gui-frontend/src/bindings.ts
```

Expected: lines showing `answerUser` command and `askUserEvent` event.

- [ ] **Step 3: Write the `AskPanel.svelte` clarification UI**

Add the `'clarifying'` state and event listener. The diff to `AskPanel.svelte`:

At the top of `<script>`, extend `PanelState`:

```typescript
type PanelState = 'loading' | 'no-api-key' | 'idle' | 'asking' | 'clarifying' | 'done' | 'error'
```

Add two new reactive variables after `errorMessage`:

```typescript
let clarifyQuestion: string = $state('')
let clarifyChoices: string[] = $state([])
let clarifyInput: string = $state('')
```

Add unlisten variable and event listener in `onMount` (alongside the others):

```typescript
let unlistenAskUser: (() => void) | undefined

onMount(async () => {
  unlistenAskUser = await events.askUserEvent.listen((event) => {
    clarifyQuestion = event.payload.question
    clarifyChoices = event.payload.choices
    clarifyInput = ''
    state = 'clarifying'
  })
  await loadSuggestions()
})

onDestroy(() => {
  unlistenAskUser?.()
  // ... existing unlisten calls
})
```

Add `sendClarification` function after `handleKeydown`:

```typescript
async function sendClarification(answer: string) {
  if (!answer.trim()) return
  state = 'asking'
  const result = await commands.answerUser(answer)
  if (result.status === 'error') {
    errorMessage = result.error
    state = 'error'
  }
  // state goes back to 'asking'; the engine will produce more tool calls or a final answer
}

function handleClarifyKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    void sendClarification(clarifyInput)
  }
}
```

- [ ] **Step 4: Add the `'clarifying'` block in the template**

In the `{#if ... {:else if ...}` chain, add after the `{:else if state === 'asking'}` block:

```svelte
{:else if state === 'clarifying'}
  <div class="message user">{lastQuestion}</div>
  <div class="clarify-box">
    <p class="clarify-question">{clarifyQuestion}</p>
    <ul class="chips">
      {#each clarifyChoices as choice, i}
        <li>
          <button class="chip" onclick={() => sendClarification(choice)}>
            <span class="chip-arrow">{i + 1}.</span> {choice}
          </button>
        </li>
      {/each}
    </ul>
    <div class="clarify-input-row">
      <input
        type="text"
        bind:value={clarifyInput}
        placeholder="Or type a custom answer…"
        onkeydown={handleClarifyKeydown}
      />
      <button
        class="send-btn"
        onclick={() => sendClarification(clarifyInput)}
        disabled={!clarifyInput.trim()}
      >
        Send
      </button>
    </div>
  </div>
```

- [ ] **Step 5: Add CSS for the clarification box**

Add to the `<style>` block:

```css
  .clarify-box {
    background: #f7f7f3;
    border: 1px solid #e0e0d8;
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .clarify-question {
    font-size: 12px;
    color: #333;
    font-weight: 500;
    margin: 0;
  }
  .clarify-input-row {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }
  .clarify-input-row input {
    flex: 1;
    padding: 6px 10px;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 12px;
    color: #333;
    background: #fafafa;
    outline: none;
    font-family: inherit;
  }
  .clarify-input-row input:focus { border-color: #2f80ed; }
```

- [ ] **Step 6: Disable the text input while clarifying**

In the bottom `{#if state !== 'no-api-key'}` input row, update the `disabled` condition:

```svelte
disabled={state === 'asking' || state === 'clarifying' || state === 'loading'}
```

And the Ask button:

```svelte
disabled={state === 'asking' || state === 'clarifying' || state === 'loading' || !question.trim()}
```

- [ ] **Step 7: Build the frontend**

```bash
cd /home/nono/dev/super-ragondin/tools/gui-frontend && npm run build 2>&1 | tail -10
```

Expected: build completes with no TypeScript errors.

- [ ] **Step 8: Run all tests**

```bash
cd /home/nono/dev/super-ragondin/tools && cargo test -q 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 9: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features 2>&1 | grep -E "^error" | head -10
```

Expected: no errors.

- [ ] **Step 10: Commit**

```bash
git add gui-frontend/src/bindings.ts gui-frontend/src/lib/AskPanel.svelte
git commit -m "feat(gui-frontend): add clarification UI for askUser tool"
```

---

## Self-Review

**Spec coverage:**
- ✅ `UserInteraction` trait in `interaction.rs` → Task 1
- ✅ `askUser(question, choices)` JS global with 2-3 choice validation → Task 2
- ✅ Tool absent when no backend → Task 3 (conditional register)
- ✅ System prompt conditional → Task 4
- ✅ Engine threads interaction → Task 5
- ✅ CLI stdin/stdout implementation → Task 6
- ✅ GUI event + managed state + `answer_user` command → Task 7
- ✅ Frontend clarification UI → Task 8
- ✅ Mock `UserInteraction` unit tests → Tasks 1, 3, 5
- ✅ `CliInteraction` `resolve_answer` tests → Task 6
- ✅ JS choice count validation (< 2 or > 3 throws) → Task 2

**Placeholders:** None found.

**Type consistency:**
- `UserInteraction::ask` → used consistently across Tasks 1–7
- `Sandbox::new` 6-arg signature → consistent across Tasks 3, 5, 6 helpers
- `AskUserEvent::NAME` → used in `GuiInteraction::ask` (tauri-specta generates this const from the struct name)
- `commands::AskUserState` → consistent between Tasks 7 and main.rs step
