# Ask Sessions Design

## Summary

Add persistent multi-turn sessions to the `ask` assistant. Sessions are saved as JSON files in `Settings/Super-Ragondin/sessions/` inside the synchronized directory, making them indexable by RAG for insights and useful for debugging.

## Session File Format

One JSON file per session, named by timestamp: `2026-03-31T14-30-00Z.json`

```json
{
  "id": "2026-03-31T14-30-00Z",
  "created_at": "2026-03-31T14:30:00Z",
  "updated_at": "2026-03-31T14:35:12Z",
  "model": "mistralai/mistral-small-2603",
  "web_search": false,
  "turns": [
    {
      "timestamp": "2026-03-31T14:30:00Z",
      "question": "What meetings do I have this week?",
      "context_dir": "work/meetings",
      "tool_calls": [
        { "code": "search('meetings this week')", "result": "[...]" },
        { "code": "getDocument('work/meetings/2026-03-31.md')", "result": "..." }
      ],
      "interactions": [
        { "question": "Which calendar?", "choices": ["Work", "Personal"], "answer": "Work" }
      ],
      "answer": "You have 3 meetings this week..."
    }
  ]
}
```

## Multi-Turn Continuation

Implicit continuation with timeout:

- Each `ask()` call checks if a recent session exists (updated within the last **30 minutes**)
- If yes, it auto-continues that session
- If not, it starts a new session
- `--new-session` flag (CLI) or "New conversation" button (GUI) forces a fresh session

## LLM Context Injection

Previous turns are injected as a condensed `[Session history]` block (last 5 turns as `Q:/A:` pairs, truncated to ~2000 chars). The full detail is preserved in the JSON file for RAG/debugging, but the LLM gets a compact recap.

The history message is inserted between the system prompt and the context message.

## Implementation

### New file: `crates/codemode/src/session.rs`

`Session` struct with:

- `Session::find_recent(sessions_dir, timeout=30min)` — scans directory for most recent `.json`, returns it if `updated_at` is within timeout
- `Session::new()` — creates fresh session with generated ID
- `Session::add_turn(turn)` — appends turn, updates `updated_at`
- `Session::save(sessions_dir)` — writes `{id}.json` to disk
- `Session::load(path)` — deserializes from JSON
- `Session::history_summary(max_turns)` — builds `[Session history]` block

Types: `Session`, `Turn`, `ToolCallRecord`, `InteractionRecord` — all derive `Serialize`/`Deserialize`.

Sessions directory: `{sync_dir}/Settings/Super-Ragondin/sessions/`

### Modified: `crates/codemode/src/engine.rs`

`CodeModeEngine::ask()` gains a `new_session: bool` parameter:

```rust
pub async fn ask(
    &self,
    question: &str,
    context_dir: Option<PathBuf>,
    web_search: bool,
    new_session: bool,
) -> Result<String>
```

- Compute `sessions_dir` from `self.sync_dir`
- Unless `new_session`, call `Session::find_recent()`
- If recent session found, inject `session.history_summary(5)` into messages
- Build `Turn` during the loop, collecting tool calls, interactions, final answer
- After loop: `session.add_turn(turn)`, `session.save(sessions_dir)`

### Modified: `crates/cli/src/main.rs`

- Add `--new-session` flag to `ask` subcommand
- Pass through to `engine.ask()`

### Modified: `crates/gui/src/commands.rs`

- Add `new_session: bool` parameter to `ask_question` Tauri command

### Modified: `gui-frontend/`

- Add "New conversation" button in ask UI
- E2E tests with screenshots for the new button

## Testing

### Unit tests (`crates/codemode/src/session.rs`)

- `Session::new()` creates valid empty session
- `Session::add_turn()` appends and updates `updated_at`
- `Session::save()` + `Session::load()` round-trip with `tempdir`
- `Session::find_recent()` returns `None` when no files / all stale
- `Session::find_recent()` returns the most recent within timeout
- `Session::history_summary()` formats correctly, truncates long histories

### Unit tests (`crates/codemode/src/engine.rs`)

- Session history injected into messages when recent session exists
- `new_session=true` ignores existing sessions

### E2E tests with screenshots (`crates/gui-e2e/`)

- "New conversation" button is visible and clickable
- Clicking it sends `new_session: true` to backend
- Screenshots captured for visual verification

### Integration tests (`crates/codemode/tests/`)

- Full ask loop with mocked LLM verifying session file is written to disk
