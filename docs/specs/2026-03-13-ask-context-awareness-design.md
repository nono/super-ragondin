# Ask Context Awareness — Design Spec

**Date:** 2026-03-13
**Branch:** improve-ask

## Problem

When the user runs `ask`, the LLM has no knowledge of:
- Where the user is working (their current directory relative to the sync dir)
- Which files have been recently touched (likely related to the question)

These two signals can meaningfully improve retrieval relevance and generated output placement.

## Goals

- Inject a lightweight context message before the user's question when relevant signals are available.
- Keep the system prompt static (`const fn`).
- No filesystem scanning — use existing store metadata.
- Future-proof: a GUI can pass `None` for `context_dir` (or a "currently open folder") without any code changes.

## Non-Goals

- Making the time window configurable (fixed at 15 minutes).
- Scanning the filesystem for recent files.
- Changing the system prompt.

## Design

### Data Flow

```
cmd_ask
  ├── std::env::current_dir().ok()  →  absolute CWD (Option<PathBuf>)
  └── CodeModeEngine::ask(question, context_dir: Option<PathBuf>)
        ├── strip_prefix(sync_dir) → relative CWD (Option<PathBuf>)
        ├── RagStore::list_recent(now - 15 min) → Vec<String> (doc_ids / paths)
        ├── build_context_message() → Option<String>
        │     None if both signals are absent/empty (no message inserted)
        └── insert as messages[1] (role: "user") before the actual question
```

### Context Message Format

When there is something to say, the context message is a `role: "user"` entry inserted as the first message after the system prompt:

```
[Context]
Current directory: work/meetings
Recently modified (last 15 min):
- work/meetings/notes.md
- work/projects/todo.md
```

Only the lines that apply are included:
- `Current directory:` only if `context_dir` resolves to a path inside `sync_dir`.
- `Recently modified:` only if `list_recent` returns at least one result.
- If neither applies, no context message is inserted (behaviour unchanged).

### Changes

#### `crates/rag/src/store.rs`

Add:

```rust
pub async fn list_recent(&self, since: SystemTime) -> Result<Vec<String>>
```

- Returns doc_ids (relative paths within sync_dir) for documents with `mtime > since`.
- `since` is converted from `SystemTime` to `i64` Unix timestamp seconds for the LanceDB filter (mtime is stored as `i64` in the LanceDB schema, and `MetadataFilter.after` already accepts `i64`).
- Uses existing LanceDB metadata filtering (same mechanism as `list_files`).
- Sorted by mtime descending (most recent first).
- Capped at 20 results to keep the context message concise.

#### `crates/codemode/src/engine.rs`

- `ask(&self, question: &str)` gains a second parameter: `context_dir: Option<PathBuf>`.
- New private async method `build_context_message(&self, context_dir: Option<PathBuf>) -> Option<String>`:
  - Strips `sync_dir` prefix from `context_dir` to get a relative path.
  - Calls `self.store.list_recent(SystemTime::now() - Duration::from_secs(900))`.
  - Returns `None` if both results are empty, otherwise returns the formatted string.
- If `Some(msg)`, inserts `{"role": "user", "content": msg}` as `messages[1]` before the question.

#### `crates/cli/src/main.rs`

- `cmd_ask` passes `std::env::current_dir().ok()` as the `context_dir` argument to `engine.ask()`.

### What Does Not Change

- `system_prompt()` remains a `const fn` with a static string.
- The sandbox, JS tools, and tool-use loop are untouched.
- No new CLI flags or environment variables.

## Testing

- Unit test for `list_recent`: insert docs with varying mtimes into a temp RagStore, verify only those within 15 min are returned.
- Unit test for `build_context_message`: cover all four combinations (no CWD + no recent, CWD only, recent only, both).
- Existing engine and sandbox tests are unaffected (pass `None` for `context_dir`).
