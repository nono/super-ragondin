# GUI Ask Assistant

**Date:** 2026-03-21
**Status:** Approved

## Overview

Replace the single-column "Synchronizing" screen in the Tauri GUI with a two-column layout: a sync status panel on the left and an ask assistant panel on the right. The assistant exposes the same `CodeModeEngine` / `SuggestionEngine` capabilities already available in the CLI `ask` command.

## Goals

- Let users ask questions about their synced files directly from the desktop app.
- Show AI-generated suggestions when no question has been asked yet.
- Store the OpenRouter API key in the app config (currently env-var-only).

## Non-goals

- Multi-turn conversation (single-shot only for now).
- Streaming or per-step progress (just a "Thinking…" spinner).
- Memos column (planned as a future third column).
- Cancellation of in-flight requests.
- A settings screen for editing the API key after initial setup (future work; users must re-run setup to change it).

---

## Architecture

### Window

`tauri.conf.json`: `width` → 760, `height` → 520, `resizable: true`, `minWidth` → 600, `minHeight` → 400.

### App state machine

`App.svelte` `Ready` state renders `MainLayout.svelte` instead of `Syncing.svelte`.

### Frontend components

| Component | Role |
|---|---|
| `MainLayout.svelte` | Flex-row wrapper, no logic |
| `SyncPanel.svelte` | Left column (~220 px fixed). Sync status badge + recent files list. |
| `AskPanel.svelte` | Right column (flex-1). Suggestions → Asking → Done state machine. |
| `Setup.svelte` | Gains an OpenRouter API key input field; call site updated to pass `api_key`. |

#### `SyncPanel.svelte`

- Subscribes to `syncStatusEvent` (already exists).
- Calls `get_recent_files()` **on mount** (to populate immediately) and again on each `Idle` event (to refresh after each sync cycle). This ensures the list is never blank at startup.
- Status badge: green "Up to date" / amber "Syncing…".
- Recent files: scrollable list of the 10 most recently modified synced files.

#### `AskPanel.svelte` — internal states

On mount, calls `get_suggestions()`. If the result is `Err("NoApiKey")`, the panel switches to the **NoApiKey** error state (banner only, no input). The `NoApiKey` detection is done lazily via the `get_suggestions()` error — `get_app_state()` is not used for this (it carries no API key information).

1. **Idle** — `get_suggestions()` succeeded. Renders suggestion chips + input field (placeholder: *"Ask anything about your files…"*, button: *"Ask"*). Clicking a chip immediately submits it as a question (transitions directly to Asking). Submitting the input also transitions directly to Asking.
2. **Asking** — displays the question and a "Thinking…" animated spinner. Awaits `ask_question(question)`.
3. **Done** — displays the question and the answer (rendered as Markdown). An input at the bottom (placeholder: *"Ask another question…"*, button: *"Ask"*) allows a new question; submitting it transitions **directly to Asking** (no re-fetch of suggestions).

---

## Backend changes

### `crates/gui/Cargo.toml`

Add `super-ragondin-codemode` as a dependency (it is not currently a dependency of the GUI crate):

```bash
cargo add super-ragondin-codemode --path ../codemode
```

### `crates/sync/src/config.rs`

Add `api_key: Option<String>` field to `Config`, annotated with `#[serde(default)]` to avoid breaking deserialization of existing config files that do not contain the field:

```rust
#[serde(default)]
pub api_key: Option<String>,
```

Note: `RagConfig` has a separate `api_key: String` field (non-optional). The `Config` field is `Option<String>` because it may not be set. When building `RagConfig`, the value is unwrapped after a `None`-check (see below).

### `crates/gui/src/commands.rs`

Three new Tauri commands added to `make_builder`. No change to `capabilities/default.json` — all custom commands are already covered by `core:default`.

```
ask_question(question: String) -> Result<String, String>
get_suggestions()              -> Result<Vec<String>, String>
get_recent_files()             -> Result<Vec<String>, String>
```

`init_config` gains a new `api_key: Option<String>` parameter. Both the public Tauri command **and** the internal helper `init_config_to(instance_url, sync_dir, config_path)` must be updated to accept and thread through the `api_key`. The `Config` struct literal inside `init_config_to` must set `api_key`. `Setup.svelte`'s call to `commands.initConfig(...)` must be updated to pass three arguments.

After changing `init_config`'s signature, regenerate the TypeScript bindings by running:

```bash
cargo test --ignored -- export_bindings
```

#### `ask_question`

1. Load `Config` from `config_path()`.
2. If `config.api_key` is `None` or empty, return `Err("NoApiKey".to_string())`.
3. Build `RagConfig`: call `RagConfig::from_env_with_db_path(config.rag_dir())`, then set `rag_config.api_key = api_key`.
4. Open `CodeModeEngine::new(rag_config, config.sync_dir, None).await`.
5. Return `engine.ask(&question, None).await.map_err(|e| e.to_string())`.

*Known limitation:* `CodeModeEngine` (and its `RagStore`) is re-created on every call. This is intentional for simplicity; caching in managed state is future work.

#### `get_suggestions`

1. Load `Config` from `config_path()`.
2. If `config.api_key` is `None` or empty, return `Err("NoApiKey".to_string())`.
3. Build `RagConfig` as above.
4. Open `SuggestionEngine::new(rag_config, config.sync_dir).await`.
5. Call `engine.generate(None).await`.
6. If the error downcasts via `err.downcast_ref::<super_ragondin_codemode::suggestions::NoFilesIndexed>()`, return `Err("NoFilesIndexed".to_string())`.
7. Otherwise propagate as `Err(e.to_string())`.

*Known limitation:* `SuggestionEngine` is re-created on every call (same rationale as above).

#### `get_recent_files`

1. Load `Config` from `config_path()`.
2. Open `TreeStore` at `config.store_dir()`.
3. Call `store.list_all_synced()`, filter to `NodeType::File` only (exclude directories — their mtime semantics are unhelpful).
4. For each file path, stat `config.sync_dir.join(rel_path)` to get filesystem mtime. If the stat fails (e.g. file deleted since last sync), **silently skip that file** — do not fail the whole command.
5. Sort by mtime descending, take top 10.
6. Return the relative path strings.
7. If loading the config or opening the store fails, return `Err(e.to_string())` (frontend silently shows empty list).

### `crates/codemode/src/engine.rs`

`CodeModeEngine::ask()` changes:

- Update the return type from `Result<()>` to `Result<String>`.
- Replace `println!("{text}"); return Ok(())` with `return Ok(text)`.
- Remove the trailing `Ok(())` after the `for` block and replace it with `unreachable!()`. The `bail!` inside the loop already handles the exhausted-iterations case; `Ok(())` was never reachable, but the compiler requires a value — `unreachable!()` makes the intent explicit.
- The CLI `cmd_ask` prints the returned string.

---

## Data flow

```
[User types / clicks chip]
        │
        ▼
AskPanel: transition → Asking
        │
        ▼
invoke ask_question(question)          [Tauri IPC]
        │
        ▼
commands::ask_question                 [Rust, async]
  └─ load Config (sync_dir + api_key)
  └─ build RagConfig (from_env_with_db_path, override api_key)
  └─ CodeModeEngine::ask()
       └─ OpenRouter tool-use loop (up to 10 iterations)
       └─ returns answer: String
        │
        ▼
AskPanel: transition → Done, render answer as Markdown
```

---

## Error handling

| Situation | Backend error string | Frontend behaviour |
|---|---|---|
| API key not configured | `"NoApiKey"` | Banner: *"Add your OpenRouter API key during setup to use the assistant."* No input shown. |
| No files indexed yet | `"NoFilesIndexed"` | *"No files indexed yet — waiting for first sync."* Input still available. |
| `ask_question` fails (other) | `e.to_string()` | Error shown inline where the answer would appear. Input remains for retry. |
| `get_recent_files` fails | any | Silently show empty list (non-critical). |
| Long answers | — | Answer area is scrollable; no truncation. |

The frontend distinguishes `"NoApiKey"` and `"NoFilesIndexed"` by exact string match on the `Err` value.

---

## UI copy reference

| Location | Text |
|---|---|
| Idle input placeholder | *"Ask anything about your files…"* |
| Idle/Done submit button | *"Ask"* |
| Thinking state | *"Thinking…"* |
| Done input placeholder | *"Ask another question…"* |
| No API key banner | *"Add your OpenRouter API key during setup to use the assistant."* |
| No files indexed | *"No files indexed yet — waiting for first sync."* |

---

## Testing

- **`init_config` with API key** — extend existing test `init_config_creates_dirs_and_config` to pass an `api_key` and verify it round-trips through the config file; also verify loading an old config JSON (without `api_key`) gives `api_key: None` (tests the `#[serde(default)]` annotation).
- **`CodeModeEngine::ask()` return value** — update existing unit tests to assert the returned `String` rather than relying on stdout.
- **`ask_question` — no API key** — unit test: config with `api_key: None` → returns `Err("NoApiKey")`.
- **`get_suggestions` — no files indexed** — unit test: empty RAG store + valid api_key → returns `Err("NoFilesIndexed")`.
- **`get_recent_files`** — unit test with a temp `TreeStore` and temp files on disk; verify returns top-10 file paths sorted by mtime descending, directories excluded, and that a missing file is silently skipped rather than causing a command error.
- **Frontend** — no automated tests (none currently exist in the project).
