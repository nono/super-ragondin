# ask-user clarification tool — design spec

**Date:** 2026-03-27

## Problem

When the user's question is ambiguous, the codemode agent currently has no way to
ask for clarification. It either guesses or produces a low-quality answer. This spec
adds an `askUser(question, choices)` JS global that pauses the tool-use loop and
prompts the user to choose from 2–3 options (or type a free-form answer).

## Goals

- The agent can propose 2–3 choices to the user mid-loop and receive an answer.
- Works in both CLI and GUI.
- When no interactive backend is available (headless/scripts/tests), the tool is
  simply absent — the LLM never sees it and cannot call it.

## Architecture

### `UserInteraction` trait (`crates/codemode/src/interaction.rs`)

```rust
pub trait UserInteraction: Send + Sync {
    /// Ask the user a question with 2–3 labelled choices.
    /// The user may pick a numbered choice or type a free-form answer.
    /// Returns the user's response as a plain string.
    fn ask(&self, question: &str, choices: &[&str]) -> String;
}
```

`CodeModeEngine` gains an `Option<Arc<dyn UserInteraction>>` field. When `Some`, the
`askUser` JS global is registered in the sandbox and the system prompt includes its
description. When `None`, neither the global nor the prompt mention exists.

### `askUser` JS global (`crates/codemode/src/tools/ask_user.rs`)

```js
askUser(question: string, choices: string[]): string
```

- `choices` must have 2–3 entries; a JS error is thrown otherwise.
- Calls `UserInteraction::ask` on the injected backend.
- Returns the user's answer as a string (either a selected choice text or free-form).

### System prompt addition (`crates/codemode/src/prompt.rs`)

Rendered only when `askUser` is available:

> "If something about the user's request is ambiguous, call
> `askUser(question, choices)` with a clear question and 2–3 labelled options.
> The user may pick a numbered option or type a free-form answer.
> Use this sparingly — only when you genuinely cannot proceed without clarification."

---

## CLI implementation (`crates/cli/`)

`CliInteraction` implements `UserInteraction`:

1. Prints the question and numbered choices to stdout.
2. Reads one line from stdin.
3. If the line is a valid number (1–N), returns the corresponding choice text.
4. Otherwise returns the raw trimmed input as free-form text.

`cmd_ask` passes `Some(Arc::new(CliInteraction))` to `CodeModeEngine::new()`.

---

## GUI implementation (`crates/gui/`)

`GuiInteraction` holds a `tauri::AppHandle` and bridges the sync sandbox thread
to the async Tauri world:

1. Emits an `ask-user` event to the frontend with `{ question, choices }`.
2. Stores a `std::sync::mpsc::Sender<String>` in Tauri managed state
   (`Mutex<Option<Sender<String>>>`).
3. Blocks the `spawn_blocking` thread on `rx.recv()`.

A new Tauri command `answer_user(answer: String)`:
- Takes the waiting `Sender` out of managed state.
- Sends the answer, unblocking the sandbox thread.

The frontend (`gui-frontend/`) listens for `ask-user`, shows an inline
question+choices form (or modal), and calls `invoke("answer_user", { answer })`
on submission.

---

## Data flow

```
User question
    │
    ▼
CodeModeEngine::ask() loop
    │  LLM calls execute_js { code: "askUser(...)" }
    ▼
Sandbox::execute()  [spawn_blocking thread]
    │  ask_user tool → UserInteraction::ask() [blocks thread]
    │
    ├─ CLI: prints to stdout, reads stdin
    └─ GUI: emits Tauri event, waits on mpsc::Receiver
                │
                │  User picks / types answer
                ▼
           answer_user Tauri command → sends on mpsc channel
                │
    ◄───────────┘
    │  askUser() returns answer string to LLM
    ▼
LLM continues with clarified intent → final answer
```

---

## Testing

### `ask_user.rs` unit tests
- Mock `UserInteraction` returning a preset string; verify JS tool calls it and
  returns the correct value.
- Verify that passing fewer than 2 or more than 3 choices throws a JS error.
- Verify that a numeric input maps to the corresponding choice text.

### `CliInteraction` unit tests
- Inject fake stdin via a test helper.
- Verify number selection (e.g. "2" → second choice text).
- Verify free-form passthrough (non-numeric input returned as-is).

### GUI / E2E
- A full GUI E2E test is out of scope for this spec.
- Manual verification: the frontend `ask-user` event and `answer_user` command
  are covered by the existing E2E test infrastructure.

---

## Out of scope

- More than 3 choices.
- Multi-step clarification trees.
- Timeout / auto-skip after N seconds.
- GUI E2E automated test for the clarification flow.
