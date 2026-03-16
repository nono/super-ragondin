# Scratchpad Tool Design

**Date:** 2026-03-16
**Status:** Approved
**Scope:** `crates/codemode`

## Summary

Add `remember(key, value)` and `recall(key)` as JavaScript globals in the codemode sandbox, providing a simple in-session key-value scratchpad for multi-step reasoning across multiple `execute_js` tool calls within a single `ask()` session.

## Motivation

Each `execute_js` tool call runs in a fresh Boa JS context. JS variables do not survive between calls. This forces the LLM to re-derive or re-fetch intermediate results on every iteration. A shared scratchpad lets the LLM store computed values (e.g., a resolved document ID, a summarized list, a running count) and retrieve them in later iterations without extra work.

## Architecture

A `Scratchpad` (`Arc<Mutex<HashMap<String, serde_json::Value>>>`) is created once per `ask()` call. It is cloned into every `Sandbox` created during that session. Inside `Sandbox::execute()` — before calling `run_boa` — the `Scratchpad` Arc is stored into the `SandboxContext` thread-local alongside the other fields (store, embedder, config, handle, sync_dir). This mirrors exactly how all existing shared state is wired.

```
ask()
 └─ scratchpad = new_scratchpad()           // Arc<Mutex<HashMap>>
     └─ per spawn_blocking call (inside the tool-call loop):
         └─ let scratchpad_clone = Arc::clone(&scratchpad);  // cloned INSIDE the loop
         └─ Sandbox::new(..., scratchpad_clone)
             └─ Sandbox::execute()
                 └─ SANDBOX_CTX.with(|cell| {
                        *cell.borrow_mut() = Some(SandboxContext {
                            ...,
                            scratchpad: Arc::clone(&self.scratchpad),
                        })
                    })
                 └─ run_boa(code)
                     └─ scratchpad::register(&mut ctx)  // reads SANDBOX_CTX inside callbacks
```

The `Arc::clone` must happen **inside** the per-tool-call loop so each closure captures its own clone rather than moving the original. Concurrent calls within one iteration all hold clones of the same `Arc`, so they share the same underlying `HashMap` safely through the mutex.

The scratchpad is dropped when `ask()` returns. It is never persisted to disk.

**Concurrency note:** When two concurrent `execute_js` calls in the same iteration write to the same key, the result is last-write-wins (the mutex serializes writes, but the order between concurrent threads is non-deterministic). The LLM prompt should warn against writing to the same key from concurrent tool calls.

## JS API

```js
// Store any JSON-serializable value under a string key
remember("user_name", "Alice")   // returns null
remember("count", 42)
remember("tags", ["a", "b"])
remember("meta", { found: true })

// Retrieve a value — returns the stored value, or null if key not found
const name = recall("user_name")  // "Alice"
const missing = recall("nope")    // null
```

- Both functions are **synchronous** (mutex lock only, no async I/O).
- `remember` always returns `null` (JS `null`).
- Keys are strings. Values are any JSON-serializable JS type: strings, numbers, booleans, arrays, objects.
- If the JS value cannot be serialized (e.g. a function, a circular reference), the key is stored with `serde_json::Value::Null` as its value. This is intentional silent degradation — the LLM prompt documents that only JSON-serializable values should be stored.
- If `serde_to_jsvalue` fails when converting a stored value back in `recall` (e.g., a number out of JS range), return `JsValue::Null` rather than throwing.

## Implementation

### New file: `crates/codemode/src/tools/scratchpad.rs`

- Defines `pub type Scratchpad = Arc<Mutex<HashMap<String, serde_json::Value>>>`.
- Exports `pub fn new_scratchpad() -> Scratchpad` (returns an empty, freshly allocated scratchpad).
- Exports `pub fn register(ctx: &mut Context) -> Result<(), JsError>`.
  - Registers `remember` and `recall` as native JS globals on `ctx`.
  - Both native callbacks access the `Scratchpad` by reading `SANDBOX_CTX` thread-local inside the callback body (same pattern used by all other tools to access `store`, `config`, etc.).
  - `remember(key, value)`:
    - Converts the JS `value` argument to `serde_json::Value` via `jsvalue_to_serde`; uses `Value::Null` on serialization failure (no exception).
    - Locks the mutex, inserts `(key, value)`, releases the lock.
    - Returns `JsValue::Null`.
    - On mutex poison: returns `Err(JsError)`.
  - `recall(key)`:
    - Locks the mutex, looks up the key.
    - If found, converts the stored `serde_json::Value` back to `JsValue` via `serde_to_jsvalue`; returns `JsValue::Null` on conversion failure.
    - If not found, returns `JsValue::Null`.
    - On mutex poison: returns `Err(JsError)`.

### Modified files

| File | Change |
|---|---|
| `tools/mod.rs` | Add `pub mod scratchpad` |
| `sandbox.rs` | Add `scratchpad: Scratchpad` field to both `SandboxContext` and `Sandbox`; `Sandbox::new()` gains a `scratchpad: Scratchpad` parameter (appended after `sync_dir`); `Sandbox::execute()` includes `scratchpad` when initializing `SandboxContext`; `run_boa` calls `tools::scratchpad::register(&mut ctx)` |
| `engine.rs` | At the top of `ask()`, call `let scratchpad = new_scratchpad()`; inside the tool-call loop, `let scratchpad_clone = Arc::clone(&scratchpad)` before each `spawn_blocking` closure, passing it to `Sandbox::new()`; update the `execute_js` tool description string to mention `remember()` and `recall()` |
| `prompt.rs` | Document `remember(key, value)` and `recall(key)` for the LLM, noting: only JSON-serializable values should be stored, and concurrent writes to the same key in the same iteration are non-deterministic |

### Existing call sites of `Sandbox::new()`

All existing call sites (tests in `sandbox.rs` and any other direct construction) must be updated to pass a `new_scratchpad()` as the new final argument. No compatibility shim is needed.

## Error Handling

- `recall` on a missing key returns JS `null` (no exception).
- Non-serializable JS values stored via `remember` silently become `null` in the map (intentional; documented in prompt).
- `serde_to_jsvalue` failure in `recall` returns `JsValue::Null`.
- Mutex poisoning returns `Err(JsError)` from the native callback, causing a JS exception.

## Testing

**Unit tests in `scratchpad.rs`** (these test the `remember`/`recall` logic in isolation by calling the Rust functions directly, without a full `Sandbox`):
- `remember` then `recall` returns the stored value (string, number, object, array).
- `recall` on a missing key returns `null`.
- Overwriting a key with a new value works correctly.

**Tests in `sandbox.rs`** (these require a full `Sandbox` and test cross-call persistence):
- Two `Sandbox::execute()` calls sharing the same `Scratchpad` instance see each other's values.
- `test_sandbox_globals_registered` extended to include `"remember"` and `"recall"`.
