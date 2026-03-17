# Design: Interactive Prompt Suggestions for `ask` Command

**Date:** 2026-03-16

## Overview

When `super-ragondin ask` is invoked with no arguments, instead of sending an empty string to the LLM, the CLI generates 6 dynamic, personalized prompt suggestions based on the user's actual files and prints them to stdout. The user copies a suggestion and re-runs `ask` with it.

Suggestions are regenerated on every invocation (no caching), biased toward creative and surprising prompts (4 creative, 2 practical).

---

## Architecture

A new `SuggestionEngine` struct lives in `crates/codemode/src/suggestions.rs`. It is separate from `CodeModeEngine` — no changes to the existing `ask` flow.

```
CLI (cmd_ask, args empty)
  └─> SuggestionEngine::new(rag_config, sync_dir)
  └─> engine.generate(cwd) -> Vec<String>
        ├─> Phase 1: data gathering (parallel, 3s timeout)
        └─> Phase 2: suggestion generation (single sub-agent call, 4s timeout)
        └─> Vec<String> (6 suggestions)
  └─> print numbered list to stdout
  └─> exit
```

`SuggestionEngine` holds:
- `rag_config: RagConfig` — for store access and model config
- `sync_dir: PathBuf` — root of synced files

`generate(cwd: Option<PathBuf>) -> Result<Vec<String>>` takes `cwd` as a parameter (not a struct field), consistent with how `CodeModeEngine::ask` takes `context_dir: Option<PathBuf>` as a parameter. The CLI passes `std::env::current_dir().ok()`. If `cwd` is `None`, skip the prefix query and go directly to the whole-store query.

`SuggestionEngine::new` is `async fn` (same as `CodeModeEngine::new`) and opens a `RagStore` internally, consistent with how `CodeModeEngine::new` does it. The CLI must call it inside the existing `rt.block_on(async { ... })`.

---

## Phase 1: Data Gathering

Goal: collect enough context about the user's files to generate specific suggestions.

**Steps:**

1. If `cwd` is `Some`, compute `path_prefix` via `cwd.strip_prefix(&sync_dir)`. If `cwd` is `None` or outside `sync_dir` (strip fails), skip to step 3.
2. Query `RagStore` for the 10 most recently modified files with `path_prefix`. If that returns 0 results, fall back to step 3.
3. Query `RagStore` for the 10 most recently modified files across the entire store (no prefix).
4. If step 3 also returns 0 results, the store is considered empty — return an `Err` signalling "no files indexed".
5. Using the files returned by whichever of steps 2 or 3 produced results: for up to 5 of those files, call `store.get_chunks(doc_id)`, concatenate chunk text (truncated to 2000 characters), and fire parallel sub-agent calls with:
   - System prompt: `"You are a helpful assistant. Summarize the following document in one sentence of at most 20 words."`
   - User prompt: the truncated text

**Timeout:** wrap the entire phase (steps 1–5) in a `tokio::time::timeout` of **3 seconds**. If the timeout fires, retain whatever summaries have already completed and treat the rest as `None`. Do not discard partial results.

**Output:** a list of up to 10 `FileContext` items, each with:
- `path: String`
- `mime_type: String`
- `summary: Option<String>` — present if sub-agent call completed before timeout

---

## Phase 2: Suggestion Generation

A single sub-agent call receives the `FileContext` list and a system prompt:

```
You are a creative assistant helping a user discover what they can ask their personal document AI.

Given the following list of recently modified files (with optional summaries), generate exactly 6 prompt suggestions.
- 2 suggestions must be practical (obvious, immediately useful)
- 4 suggestions must be creative, surprising, or delightful — things the user would not think to ask on their own
- Each suggestion must be specific to the actual content provided, not generic
- Each suggestion must be under 80 characters
- Return ONLY a JSON array of 6 strings, with no other text or explanation
```

The user prompt contains the serialized `FileContext` list as JSON, using field names `path`, `mime_type`, and `summary` (null if unavailable). Example:
```json
[
  {"path": "work/meetings/2026-03-10.md", "mime_type": "text/plain", "summary": "Notes from the Q1 planning meeting."},
  {"path": "photos/trip.jpg", "mime_type": "image/jpeg", "summary": null}
]
```

**Parse & retry:** parse the response as `Vec<String>` with exactly 6 elements.
- If parsing fails, send a **corrective follow-up turn** appending: `"Your response was not valid JSON. Return ONLY a JSON array of 6 strings."` and retry once.
- If the second attempt also fails, return an `Err`.

**Timeout:** the first Phase 2 call has a **4s** `tokio::time::timeout`. If parsing fails and there is time remaining (i.e. the timeout has not yet fired), the corrective retry is attempted within the same timeout window. In practice, a slow first call may leave insufficient time for the retry — this is an accepted trade-off given the 5s total budget. If the timeout fires at any point, return an `Err`.

---

## CLI Integration

In `cmd_ask`, before building the question:

```rust
if args.is_empty() {
    // run SuggestionEngine, print, exit
}
```

Output format:
```
Not sure what to ask? Here are some ideas:

1. <suggestion>
2. <suggestion>
3. <suggestion>
4. <suggestion>
5. <suggestion>
6. <suggestion>
```

No changes to the non-empty args path.

---

## Error Handling

| Situation | Behavior |
|---|---|
| Phase 1 timeout (>3s) | Use partial summaries already completed; continue to Phase 2 |
| Phase 2 JSON parse fails twice | Print `"Could not generate suggestions. Try: super-ragondin ask <your question>"` and exit |
| Phase 2 timeout (>4s) | Print `"Could not generate suggestions. Try: super-ragondin ask <your question>"` and exit |
| RagStore has no indexable files | Print `"No files indexed yet. Run super-ragondin sync first."` and exit |
| `OPENROUTER_API_KEY` not set | Same error as current `ask` command |

"No indexable files" means `list_docs` with no filter returns 0 results (files that were skipped during indexing and appear only in `skipped_docs` do not count).

---

## Testing

| Test | Type | Notes |
|---|---|---|
| `cwd` inside sync_dir → prefix query used | Unit | Mock store returning results for prefix |
| `cwd` outside sync_dir → whole-store query used | Unit | strip_prefix fails → skip prefix query |
| Prefix query returns 0 → whole-store fallback | Unit | Mock store returning empty for prefix, non-empty for whole |
| Whole-store query returns 0 → empty error | Unit | Mock store returning empty |
| Phase 1 partial timeout → completed summaries retained | Unit | Mock slow sub-agent for some files |
| Phase 2 bad JSON → corrective retry → friendly error | Integration (ignored) | Requires API key; retry path needs injected HTTP or real call |
| Full flow returns 6 strings under 5s | Integration (ignored) | Requires API key |
