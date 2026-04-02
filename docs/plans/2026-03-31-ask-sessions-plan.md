# Ask Sessions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add persistent multi-turn sessions to the ask assistant, stored as JSON in the sync directory.

**Architecture:** A new `session.rs` module in `crates/codemode/` handles session persistence (load/save/find_recent). The engine builds `Turn` structs during the tool-use loop and appends them to the session. Previous turns are injected as a condensed `[Session history]` block. CLI gets `--new-session`, GUI gets a "New conversation" button.

**Tech Stack:** Rust (serde, chrono), Svelte 5, Tauri v2 (tauri-specta), thirtyfour (E2E)

---

### Task 1: Session data types and basic tests

**Files:**
- Create: `crates/codemode/src/session.rs`
- Modify: `crates/codemode/src/lib.rs`

**Step 1: Add `chrono` dependency to codemode crate**

Run: `cargo add chrono --features serde -p super-ragondin-codemode`

**Step 2: Create `session.rs` with data types (red — tests first)**

Create `crates/codemode/src/session.rs` with types and test stubs:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub code: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRecord {
    pub question: String,
    pub choices: Vec<String>,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub timestamp: DateTime<Utc>,
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_dir: Option<String>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub interactions: Vec<InteractionRecord>,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub web_search: bool,
    pub turns: Vec<Turn>,
}
```

**Step 3: Register module in `lib.rs`**

In `crates/codemode/src/lib.rs`, add:
```rust
pub mod session;
```

**Step 4: Write failing tests for `Session::new()`**

In `session.rs`, add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_has_empty_turns() {
        let session = Session::new("test-model", false);
        assert!(session.turns.is_empty());
        assert_eq!(session.model, "test-model");
        assert!(!session.web_search);
    }

    #[test]
    fn new_session_id_is_timestamp_format() {
        let session = Session::new("m", false);
        // ID should match YYYY-MM-DDTHH-MM-SSZ pattern
        assert!(session.id.ends_with('Z'), "id={}", session.id);
        assert!(session.id.len() >= 20, "id={}", session.id);
    }
}
```

**Step 5: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-codemode session::tests -- -q`
Expected: FAIL — `Session::new` not found.

**Step 6: Implement `Session::new()`**

```rust
impl Session {
    #[must_use]
    pub fn new(model: &str, web_search: bool) -> Self {
        let now = Utc::now();
        let id = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
        Self {
            id,
            created_at: now,
            updated_at: now,
            model: model.to_string(),
            web_search,
            turns: Vec::new(),
        }
    }
}
```

**Step 7: Run tests to verify they pass**

Run: `cargo test -p super-ragondin-codemode session::tests -- -q`
Expected: PASS

**Step 8: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-features`

**Step 9: Commit**

```bash
git add crates/codemode/src/session.rs crates/codemode/src/lib.rs crates/codemode/Cargo.toml Cargo.lock
git commit -m "feat(codemode): add Session data types with Session::new()"
```

---

### Task 2: Session add_turn, save, load

**Files:**
- Modify: `crates/codemode/src/session.rs`

**Step 1: Write failing tests for `add_turn`**

```rust
#[test]
fn add_turn_appends_and_updates_timestamp() {
    let mut session = Session::new("m", false);
    let before = session.updated_at;
    std::thread::sleep(std::time::Duration::from_millis(10));
    let turn = Turn {
        timestamp: Utc::now(),
        question: "hello".to_string(),
        context_dir: None,
        tool_calls: vec![],
        interactions: vec![],
        answer: "world".to_string(),
    };
    session.add_turn(turn);
    assert_eq!(session.turns.len(), 1);
    assert!(session.updated_at >= before);
    assert_eq!(session.turns[0].question, "hello");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode session::tests::add_turn -- -q`
Expected: FAIL

**Step 3: Implement `add_turn`**

```rust
pub fn add_turn(&mut self, turn: Turn) {
    self.updated_at = Utc::now();
    self.turns.push(turn);
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p super-ragondin-codemode session::tests::add_turn -- -q`
Expected: PASS

**Step 5: Write failing tests for `save` and `load` round-trip**

```rust
#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new("test-model", true);
    session.add_turn(Turn {
        timestamp: Utc::now(),
        question: "q1".to_string(),
        context_dir: Some("work/docs".to_string()),
        tool_calls: vec![ToolCallRecord {
            code: "search('hi')".to_string(),
            result: "[]".to_string(),
        }],
        interactions: vec![InteractionRecord {
            question: "which?".to_string(),
            choices: vec!["a".to_string(), "b".to_string()],
            answer: "a".to_string(),
        }],
        answer: "a1".to_string(),
    });
    session.save(dir.path()).unwrap();
    let path = dir.path().join(format!("{}.json", session.id));
    assert!(path.exists());
    let loaded = Session::load(&path).unwrap();
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.turns.len(), 1);
    assert_eq!(loaded.turns[0].question, "q1");
    assert_eq!(loaded.turns[0].tool_calls.len(), 1);
    assert_eq!(loaded.turns[0].interactions.len(), 1);
    assert!(loaded.web_search);
}
```

**Step 6: Run test to verify it fails**

Run: `cargo test -p super-ragondin-codemode session::tests::save_and_load -- -q`
Expected: FAIL

**Step 7: Implement `save` and `load`**

```rust
use std::path::Path;
use anyhow::{Context as _, Result};

impl Session {
    pub fn save(&self, sessions_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(sessions_dir)
            .context("failed to create sessions directory")?;
        let path = sessions_dir.join(format!("{}.json", self.id));
        let json = serde_json::to_string_pretty(self)
            .context("failed to serialize session")?;
        std::fs::write(&path, json)
            .context("failed to write session file")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .context("failed to read session file")?;
        let session: Self = serde_json::from_str(&data)
            .context("failed to deserialize session")?;
        Ok(session)
    }
}
```

**Step 8: Run tests, format, lint**

Run: `cargo test -p super-ragondin-codemode session::tests -- -q && cargo fmt --all && cargo clippy --all-features`
Expected: PASS, no warnings

**Step 9: Commit**

```bash
git add crates/codemode/src/session.rs
git commit -m "feat(codemode): add Session save/load and add_turn"
```

---

### Task 3: Session find_recent

**Files:**
- Modify: `crates/codemode/src/session.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn find_recent_returns_none_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let result = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800));
    assert!(result.unwrap().is_none());
}

#[test]
fn find_recent_returns_none_when_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new("m", false);
    // Backdate updated_at by 1 hour
    session.updated_at = Utc::now() - chrono::Duration::hours(1);
    session.save(dir.path()).unwrap();
    let result = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800));
    assert!(result.unwrap().is_none());
}

#[test]
fn find_recent_returns_most_recent() {
    let dir = tempfile::tempdir().unwrap();
    let mut s1 = Session::new("m", false);
    s1.id = "2026-01-01T00-00-00Z".to_string();
    s1.save(dir.path()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let s2 = Session::new("m", false);
    s2.save(dir.path()).unwrap();
    let found = Session::find_recent(dir.path(), std::time::Duration::from_secs(1800))
        .unwrap()
        .unwrap();
    assert_eq!(found.id, s2.id);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-codemode session::tests::find_recent -- -q`
Expected: FAIL

**Step 3: Implement `find_recent`**

```rust
impl Session {
    pub fn find_recent(sessions_dir: &Path, timeout: std::time::Duration) -> Result<Option<Self>> {
        let entries = match std::fs::read_dir(sessions_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("failed to read sessions directory"),
        };

        let cutoff = Utc::now() - chrono::Duration::from_std(timeout)
            .unwrap_or_else(|_| chrono::Duration::seconds(1800));

        let mut best: Option<Session> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(session) = Self::load(&path) {
                if session.updated_at > cutoff {
                    if best.as_ref().is_none_or(|b| session.updated_at > b.updated_at) {
                        best = Some(session);
                    }
                }
            }
        }
        Ok(best)
    }
}
```

**Step 4: Run tests, format, lint**

Run: `cargo test -p super-ragondin-codemode session::tests -- -q && cargo fmt --all && cargo clippy --all-features`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/codemode/src/session.rs
git commit -m "feat(codemode): add Session::find_recent with timeout"
```

---

### Task 4: Session history_summary

**Files:**
- Modify: `crates/codemode/src/session.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn history_summary_empty_session() {
    let session = Session::new("m", false);
    let summary = session.history_summary(5);
    assert!(summary.is_none());
}

#[test]
fn history_summary_formats_turns() {
    let mut session = Session::new("m", false);
    session.add_turn(Turn {
        timestamp: Utc::now(),
        question: "What is X?".to_string(),
        context_dir: None,
        tool_calls: vec![],
        interactions: vec![],
        answer: "X is Y.".to_string(),
    });
    session.add_turn(Turn {
        timestamp: Utc::now(),
        question: "And Z?".to_string(),
        context_dir: None,
        tool_calls: vec![],
        interactions: vec![],
        answer: "Z is W.".to_string(),
    });
    let summary = session.history_summary(5).unwrap();
    assert!(summary.contains("[Session history]"));
    assert!(summary.contains("Q: What is X?"));
    assert!(summary.contains("A: X is Y."));
    assert!(summary.contains("Q: And Z?"));
    assert!(summary.contains("A: Z is W."));
}

#[test]
fn history_summary_limits_to_max_turns() {
    let mut session = Session::new("m", false);
    for i in 0..10 {
        session.add_turn(Turn {
            timestamp: Utc::now(),
            question: format!("q{i}"),
            context_dir: None,
            tool_calls: vec![],
            interactions: vec![],
            answer: format!("a{i}"),
        });
    }
    let summary = session.history_summary(3).unwrap();
    // Should only contain the last 3 turns (q7, q8, q9)
    assert!(!summary.contains("Q: q0"));
    assert!(!summary.contains("Q: q6"));
    assert!(summary.contains("Q: q7"));
    assert!(summary.contains("Q: q8"));
    assert!(summary.contains("Q: q9"));
}

#[test]
fn history_summary_truncates_long_answers() {
    let mut session = Session::new("m", false);
    let long_answer = "x".repeat(3000);
    session.add_turn(Turn {
        timestamp: Utc::now(),
        question: "q".to_string(),
        context_dir: None,
        tool_calls: vec![],
        interactions: vec![],
        answer: long_answer,
    });
    let summary = session.history_summary(5).unwrap();
    assert!(summary.len() < 2500, "summary too long: {}", summary.len());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p super-ragondin-codemode session::tests::history_summary -- -q`
Expected: FAIL

**Step 3: Implement `history_summary`**

```rust
const MAX_SUMMARY_CHARS: usize = 2000;
const MAX_ANSWER_CHARS: usize = 500;

impl Session {
    #[must_use]
    pub fn history_summary(&self, max_turns: usize) -> Option<String> {
        if self.turns.is_empty() {
            return None;
        }
        let start = self.turns.len().saturating_sub(max_turns);
        let mut lines = vec!["[Session history]".to_string()];
        let mut total_len = lines[0].len();
        for turn in &self.turns[start..] {
            let q_line = format!("Q: {}", turn.question);
            let answer = if turn.answer.len() > MAX_ANSWER_CHARS {
                format!("{}…", &turn.answer[..MAX_ANSWER_CHARS])
            } else {
                turn.answer.clone()
            };
            let a_line = format!("A: {answer}");
            total_len += q_line.len() + a_line.len() + 2; // newlines
            if total_len > MAX_SUMMARY_CHARS {
                break;
            }
            lines.push(q_line);
            lines.push(a_line);
        }
        Some(lines.join("\n"))
    }
}
```

Note: the `MAX_ANSWER_CHARS` truncation must respect char boundaries. Use `turn.answer.char_indices()` to find a safe boundary if needed, or use `floor_char_boundary` if available. The implementer should handle this.

**Step 4: Run tests, format, lint**

Run: `cargo test -p super-ragondin-codemode session::tests -- -q && cargo fmt --all && cargo clippy --all-features`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/codemode/src/session.rs
git commit -m "feat(codemode): add Session::history_summary"
```

---

### Task 5: Engine integration — wire sessions into `ask()`

**Files:**
- Modify: `crates/codemode/src/engine.rs`

**Step 1: Add `new_session` parameter to `ask()` signature**

Change the `ask` method signature in `crates/codemode/src/engine.rs` (line ~128):

```rust
pub async fn ask(
    &self,
    question: &str,
    context_dir: Option<std::path::PathBuf>,
    web_search: bool,
    new_session: bool,
) -> Result<String> {
```

**Step 2: Add session logic at the start of `ask()`**

After the `tracing::info!` line and before building `messages`, add:

```rust
use crate::session::{Session, Turn, ToolCallRecord, InteractionRecord};

let sessions_dir = self.sync_dir.join("Settings/Super-Ragondin/sessions");
let mut session = if new_session {
    Session::new(model, web_search)
} else {
    Session::find_recent(&sessions_dir, std::time::Duration::from_secs(1800))?
        .unwrap_or_else(|| Session::new(model, web_search))
};
```

**Step 3: Inject history summary into messages**

After the system prompt message and before the context message, add:

```rust
if let Some(history) = session.history_summary(5) {
    messages.push(serde_json::json!({"role": "user", "content": history}));
}
```

**Step 4: Collect tool calls and interactions during the loop**

Add mutable collectors before the loop:

```rust
let mut turn_tool_calls: Vec<ToolCallRecord> = Vec::new();
let mut turn_interactions: Vec<InteractionRecord> = Vec::new();
```

Inside the tool call execution block, after getting the result, record it:

```rust
turn_tool_calls.push(ToolCallRecord {
    code: tool_call.code.clone(),
    result: tool_result.clone(),
});
```

Note: `InteractionRecord` collection requires the `UserInteraction` trait to report interactions back. This is a stretch — for now, interactions are not captured (they'd require modifying the trait). Add a `// TODO: capture interactions when UserInteraction trait supports it` comment.

**Step 5: Save session after getting the final answer**

In the `else if let Some(text) = extract_text(...)` block, before the `return Ok(text)`:

```rust
let context_dir_str = context_dir.as_ref().and_then(|p| {
    p.strip_prefix(&self.sync_dir).ok().map(|r| r.to_string_lossy().into_owned())
});
let turn = Turn {
    timestamp: chrono::Utc::now(),
    question: question.to_string(),
    context_dir: context_dir_str,
    tool_calls: turn_tool_calls,
    interactions: turn_interactions,
    answer: text.clone(),
};
session.add_turn(turn);
if let Err(e) = session.save(&sessions_dir) {
    tracing::warn!(error = %e, "failed to save session");
}
```

**Step 6: Fix all compilation errors**

Update all call sites that call `engine.ask()` to pass the new `new_session` parameter:

- `crates/cli/src/main.rs:379` — add `false` (or use the CLI flag, see Task 6)
- `crates/gui/src/commands.rs:457` — add `false` (or use the GUI param, see Task 7)
- `crates/gui/src/commands.rs:381` — `ask_question_from` helper — add `false`
- Any tests in `engine.rs` that call `ask()` — add `false`

For now, pass `false` everywhere to get compilation working. Tasks 6 and 7 will wire the proper flags.

**Step 7: Run tests, format, lint**

Run: `cargo test -p super-ragondin-codemode -- -q && cargo fmt --all && cargo clippy --all-features`
Expected: PASS

**Step 8: Commit**

```bash
git add crates/codemode/src/engine.rs crates/cli/src/main.rs crates/gui/src/commands.rs
git commit -m "feat(codemode): wire sessions into ask() loop"
```

---

### Task 6: CLI `--new-session` flag

**Files:**
- Modify: `crates/cli/src/main.rs`

**Step 1: Parse `--new-session` flag in `cmd_ask`**

In `cmd_ask()` (line ~312), after the `--web` parsing, add `--new-session` parsing:

```rust
let new_session = args.iter().any(|a| a == "--new-session");
let question_args: Vec<&str> = args
    .iter()
    .filter(|a| *a != "--web" && *a != "--new-session")
    .map(String::as_str)
    .collect();
```

Note: the existing `--web` parsing uses `skip(usize::from(web_search))` which assumes `--web` is the first arg. Refactor both flags to use a filter approach instead.

**Step 2: Pass `new_session` to `engine.ask()`**

Change line ~379:
```rust
let answer = engine
    .ask(&question, cwd, web_search, new_session)
    .await
```

**Step 3: Run build to verify**

Run: `cargo build -p super-ragondin-cli && cargo fmt --all && cargo clippy --all-features`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): add --new-session flag to ask command"
```

---

### Task 7: GUI `new_session` parameter

**Files:**
- Modify: `crates/gui/src/commands.rs`
- Modify: `gui-frontend/src/bindings.ts` (regenerated)
- Modify: `gui-frontend/src/lib/AskPanel.svelte`

**Step 1: Add `new_session` param to `ask_question` command**

In `crates/gui/src/commands.rs`, modify `ask_question` (line ~439):

```rust
pub async fn ask_question(
    question: String,
    web_search: bool,
    new_session: bool,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
```

And update the `engine.ask()` call:
```rust
engine
    .ask(&question, None, web_search, new_session)
    .await
```

Also update `ask_question_from` to accept and pass `new_session`:
```rust
pub async fn ask_question_from(
    question: &str,
    config_path: &std::path::Path,
) -> Result<String, String> {
    // ...
    engine.ask(question, None, false, false).await
}
```

**Step 2: Regenerate TypeScript bindings**

Run: `cargo test -p super-ragondin-gui export_bindings -- --ignored`

This updates `gui-frontend/src/bindings.ts` so `askQuestion` accepts `newSession` as a third parameter.

**Step 3: Add "New conversation" button to `AskPanel.svelte`**

In `gui-frontend/src/lib/AskPanel.svelte`:

Add a state variable:
```typescript
let newSession: boolean = $state(false)
```

Update the `ask` function to pass and reset it:
```typescript
async function ask(q: string) {
    if (!q.trim()) return
    lastQuestion = q
    question = ''
    panelState = 'asking'
    const forceNew = newSession
    newSession = false
    const result = await commands.askQuestion(q, webSearch, forceNew)
    // ...
}
```

Add a "New conversation" button in the `input-row` div, next to the web search toggle:
```svelte
<button
  class="new-session-btn"
  onclick={() => { newSession = true; /* visual feedback */ }}
  disabled={panelState === 'asking' || panelState === 'clarifying' || panelState === 'loading'}
  title="Start a new conversation"
>
  ✦ New
</button>
```

Add styling for the button:
```css
.new-session-btn {
    padding: 6px 10px;
    background: none;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 11px;
    color: #666;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
}
.new-session-btn:hover { background: #f5f5f5; border-color: #bbb; }
.new-session-btn:disabled { opacity: 0.5; cursor: not-allowed; }
```

**Step 4: Build frontend and verify**

Run (from workspace root):
```bash
cd gui-frontend && npm run build && cd ..
cargo build -p super-ragondin-gui --no-default-features --features custom-protocol
cargo fmt --all && cargo clippy --all-features
```

**Step 5: Commit**

```bash
git add crates/gui/src/commands.rs gui-frontend/src/bindings.ts gui-frontend/src/lib/AskPanel.svelte
git commit -m "feat(gui): add new_session parameter and New conversation button"
```

---

### Task 8: E2E test for New conversation button

**Files:**
- Create: `crates/gui-e2e/tests/ask_new_session_button.rs`

**Step 1: Write E2E test**

Follow the pattern from `crates/gui-e2e/tests/ask_chip_interaction.rs`. The test should:

1. Set up a config in Ready state (with fake tokens)
2. Start tauri-driver and connect
3. Wait for the Ask panel to render
4. Find the "New conversation" button (`.new-session-btn`)
5. Verify it exists and is not disabled
6. Click it
7. Take a screenshot
8. Compare against baseline (use `compare_or_create_baseline`)

```rust
use gui_e2e::{
    app_binary_path, compare_or_create_baseline, connect_driver, references_dir,
    save_screenshot, screenshots_dir, start_tauri_driver, ConfigGuard, TauriDriverGuard,
};
use thirtyfour::prelude::*;

#[tokio::test]
#[ignore = "requires built GUI binary and tauri-driver"]
async fn new_session_button_is_visible_and_clickable() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    let sync_dir = tempfile::tempdir().expect("failed to create temp sync dir");
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");

    let ready_config = format!(
        r#"{{
  "instance_url": "https://alice.mycozy.cloud",
  "sync_dir": "{}",
  "data_dir": "{}",
  "api_key": "fake-openrouter-key",
  "oauth_client": {{
    "instance_url": "https://alice.mycozy.cloud",
    "client_id": "fake-client-id",
    "client_secret": "fake-client-secret",
    "registration_access_token": "fake-reg-token",
    "access_token": "fake-access-token",
    "refresh_token": null
  }},
  "last_seq": null
}}"#,
        sync_dir.path().display(),
        data_dir.path().display()
    );

    let _config_guard = ConfigGuard::install(&ready_config);

    let _tauri_driver = TauriDriverGuard::new(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;
    driver.goto("tauri://localhost").await.ok();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Ask panel to render.
    driver
        .query(By::Css(".panel-header .title"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;

    // Find the New conversation button.
    let new_btn = driver
        .query(By::Css(".new-session-btn"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;

    // Verify it's not disabled.
    let disabled = new_btn.attr("disabled").await?;
    assert!(disabled.is_none(), "button should not be disabled");

    // Click it.
    new_btn.click().await?;

    // Take screenshot.
    let screenshot_path = screenshots_dir().join("ask_new_session_button.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    // Compare against baseline.
    let reference_path = references_dir().join("ask_new_session_button.png");
    if let Err(msg) = compare_or_create_baseline(&screenshot_path, &reference_path, 5.0) {
        if msg.contains("Baseline created") {
            eprintln!("{msg}");
        } else {
            panic!("{msg}");
        }
    }

    driver.quit().await?;
    Ok(())
}
```

**Step 2: Build and run E2E test**

Run:
```bash
cargo build -p super-ragondin-gui --no-default-features --features custom-protocol
xvfb-run cargo test -p gui-e2e ask_new_session_button -- --ignored
```

Expected: First run creates baseline and fails with "Baseline created — review and commit it". Second run should PASS.

**Step 3: Commit**

```bash
git add crates/gui-e2e/tests/ask_new_session_button.rs crates/gui-e2e/references/
git commit -m "test(gui-e2e): add E2E test for New conversation button"
```

---

### Task 9: Final verification and documentation

**Files:**
- Modify: `docs/guides/rag.md`

**Step 1: Run full test suite**

Run: `cargo test -q && cargo fmt --all && cargo clippy --all-features`
Expected: PASS

**Step 2: Update the RAG guide**

Add to `docs/guides/rag.md` under the `crates/codemode/` section:

```markdown
- `src/session.rs` - `Session` — persistent multi-turn session: save/load JSON, `find_recent()` with 30-min timeout, `history_summary()` for LLM context injection
```

Add a finding:
```markdown
- Sessions are stored as JSON files in `{sync_dir}/Settings/Super-Ragondin/sessions/` — one file per session, named by UTC timestamp (e.g. `2026-03-31T14-30-00Z.json`). They are indexed by RAG for insights.
```

**Step 3: Commit**

```bash
git add docs/guides/rag.md
git commit -m "docs: document ask sessions in RAG guide"
```
