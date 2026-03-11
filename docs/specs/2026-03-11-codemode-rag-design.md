# Code Mode RAG Design

**Date:** 2026-03-11
**Status:** Approved

## Overview

Replace the current `ask` command's simple search-then-generate pipeline with a code-mode approach: the LLM receives a JS sandbox tool and can execute arbitrary JavaScript to query the vector database, discover files, and call sub-agents before constructing its final answer.

This enables the LLM to decompose complex questions into multiple targeted sub-queries, summarize results per subdomain via sub-agents, and synthesize a comprehensive answer — rather than being limited to a single top-K vector search.

## Architecture

New crate `crates/codemode/` depending on `super-ragondin-rag`:

```
crates/codemode/
  src/
    lib.rs            - public CodeModeEngine
    engine.rs         - LLM tool-use loop
    sandbox.rs        - Boa JS context + registered functions
    tools/
      search.rs       - search() JS function → RagStore vector search
      list_files.rs   - listFiles() JS function → RagStore metadata query
      get_document.rs - getDocument() JS function → all chunks for a doc
      sub_agent.rs    - subAgent() JS function → OpenRouter chat completion
```

The CLI `ask` command calls `CodeModeEngine::ask(question)` and streams the final answer to stdout.

## JS Sandbox API

The LLM is given a single tool: `execute_js(code: string)`. The code runs in a fresh Boa JS context per call. The return value is the last expression, JSON-serialized.

Four globals are available inside the sandbox:

```js
// Semantic vector search with optional metadata filters
search(query, options?)
// options: { limit?: number, mimeType?: string, pathPrefix?: string, after?: string, before?: string }
// returns: [{ doc_id, chunk_text, mime_type, mtime }, ...]
// mtime is an ISO 8601 string (e.g. "2024-06-15T10:30:00Z")
// chunk_index is intentionally omitted — chunk order is not relevant for search results

// Discover files by metadata
listFiles(options?)
// options: { sort?: "recent" | "oldest", limit?: number, mimeType?: string, pathPrefix?: string, after?: string, before?: string }
// returns: [{ doc_id, mime_type, mtime }, ...]
// mtime is an ISO 8601 string
// after/before accept ISO 8601 strings (date or datetime)
// limit applies after de-duplication and sorting

// Fetch all chunks from a specific document in order
getDocument(docId)
// returns: [{ chunk_index, chunk_text }, ...] sorted by chunk_index

// Call the LLM as a sub-agent (one-shot, no tools)
subAgent(systemPrompt, userPrompt)
// returns: string
```

JS code can do arbitrary computation between calls — filtering, merging, ranking, formatting — before returning a value to the LLM.

### mtime format

All `mtime` values exposed to JS are ISO 8601 strings (UTC). The `after`/`before` filter options also accept ISO 8601 strings. Conversion to/from LanceDB's internal `i64` Unix seconds is done entirely in Rust, not in JS.

### Example

For "Give me 5 key points from the note I just added", the LLM might generate:

```js
const files = listFiles({ sort: "recent", limit: 1 });
const chunks = getDocument(files[0].doc_id);
chunks.map(c => c.chunk_text).join("\n\n")
```

For a multi-subdomain question, it might run separate searches and summarize each:

```js
const topicsA = search("budget forecasts", { limit: 3 });
const topicsB = search("team headcount", { limit: 3 });
const summaryA = subAgent("Summarize concisely.", topicsA.map(r => r.chunk_text).join("\n"));
const summaryB = subAgent("Summarize concisely.", topicsB.map(r => r.chunk_text).join("\n"));
({ budget: summaryA, headcount: summaryB })
```

## System Prompt

Lives in `crates/codemode/src/prompt.rs` as a `system_prompt() -> &'static str` function, isolated from engine logic for easy modification.

```
You are Super Ragondin, a helpful assistant with access to a personal document database.
To answer questions, use the `execute_js` tool to query the database before responding.

Available JavaScript functions:

  search(query, options?)
    Semantic vector search. Options: { limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, chunk_text, mime_type, mtime }, ...]

  listFiles(options?)
    Discover files by metadata. Options: { sort: "recent"|"oldest", limit, mimeType, pathPrefix, after, before }
    Returns: [{ doc_id, mime_type, mtime }, ...]

  getDocument(docId)
    Fetch all chunks of a document in order.
    Returns: [{ chunk_index, chunk_text }, ...]

  subAgent(systemPrompt, userPrompt)
    Ask a fast LLM to process text (summarize, extract, etc.)
    Returns: string

Rules:
- Each execute_js call is a fresh context — variables do not persist between calls
- The last expression is the return value (JSON-serialized)
- Dates (mtime, after, before) are ISO 8601 strings
- Use multiple calls when gathering information in stages
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
```

## LLM Tool-Use Loop

```
ask(question)
│
├── Load system prompt from prompt.rs
├── Send to LLM: [system, user: question] + execute_js tool definition
│
└── Loop (max 10 iterations):
    ├── LLM responds with tool_call → execute_js(code)
    │   ├── Run code in fresh Boa context (5s timeout)
    │   ├── Capture result (last expression, JSON-serialized)
    │   ├── On JS error or Rust error → return error string (LLM can retry/adapt)
    │   └── Append tool_call + tool_result to message history
    │
    └── LLM responds with text (no tool call)
        └── Stream final answer to stdout
```

Tool-call phase uses non-streaming OpenRouter requests (full JSON response needed to extract tool calls). The final text response is streamed to stdout as today.

**Safety limits:**
- Max 10 tool-call iterations per query
- 5-second JS execution timeout per `execute_js` call
- JS errors and Rust errors inside tools are caught and returned as error strings — the tool-use loop never hard-aborts due to a tool failure

### Error handling contract

Errors are surfaced to the LLM as strings so it can retry or adapt:

| Error source | Behaviour |
|---|---|
| JS syntax/runtime error | Boa exception caught → `"JS error: <message>"` returned as tool result |
| LanceDB query failure | `Err` propagated to Boa host function → thrown as JS exception → caught as above |
| OpenRouter call failure in `subAgent` | `Err` propagated to Boa host function → thrown as JS exception → caught as above |

The tool-use loop itself only hard-fails if the max iteration limit is reached (returns a fixed error message to the CLI).

## Data Flow & Integration

### Configuration

`RagConfig` gains one new env var:

| Variable | Default | Description |
|---|---|---|
| `OPENROUTER_SUBAGENT_MODEL` | `google/gemini-2.5-flash` | Model used for `subAgent()` calls — can be a cheaper/faster model than the main chat model |

The main reasoning model (tool-use loop + final answer) continues to use `OPENROUTER_CHAT_MODEL`. Sub-agents are simple summarization tasks that don't require the same capability, so a smaller model keeps costs low.

### Changes to existing code

- `crates/cli/src/main.rs`: `cmd_ask()` replaced — loads `RagConfig`, constructs `CodeModeEngine`, calls `engine.ask(question)`
- `crates/rag/src/config.rs`: add `subagent_model` field loaded from `OPENROUTER_SUBAGENT_MODEL`
- `crates/rag/src/store.rs`:
  - `search()` gains an optional `MetadataFilter` parameter, threaded all the way into the LanceDB `vector_search()` call as a `.only_if(where_clause)` predicate
  - New `list_docs(filter: Option<MetadataFilter>, sort: DocSort) -> Vec<DocInfo>` method returning one entry per unique `doc_id`, ordered by `mtime`
  - New `get_chunks(doc_id: &str) -> Vec<ChunkInfo>` method returning all chunks for a doc ordered by `chunk_index`
- `Cargo.toml` workspace: add `crates/codemode` member

### New types in `crates/rag/src/store.rs`

```rust
/// Filter applied server-side in LanceDB for both search() and list_docs()
pub struct MetadataFilter {
    pub mime_type: Option<String>,   // exact match: mime_type = '...'
    pub path_prefix: Option<String>, // doc_id LIKE 'prefix/%' (trailing slash added by Rust if absent)
    pub after: Option<DateTime<Utc>>,  // mtime > unix_seconds
    pub before: Option<DateTime<Utc>>, // mtime < unix_seconds
}

/// Sort order for list_docs()
pub enum DocSort { Recent, Oldest }

/// One entry per document returned by list_docs()
pub struct DocInfo {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: DateTime<Utc>,
}

/// One chunk entry returned by get_chunks()
pub struct ChunkInfo {
    pub chunk_index: u32,
    pub chunk_text: String,
}
```

### MetadataFilter construction

The JS options object (e.g. `{ mimeType: "application/pdf", pathPrefix: "work/", after: "2024-01-01" }`) is deserialized by the Rust tool handler into a typed `MetadataFilter` struct. The struct is then converted to a LanceDB `WHERE` clause in Rust. JS code never constructs SQL strings — this prevents injection.

Each field is independently validated and escaped before composition:
- `path_prefix`: trailing slash appended if absent; value is SQL-escaped before insertion into `doc_id LIKE 'prefix/%'`
- `mime_type`: SQL-escaped, matched with `=`
- `after`/`before`: converted from `DateTime<Utc>` to `i64` Unix seconds, used with `>` / `<`

### New dependencies for `crates/codemode/`

| Crate | Purpose |
|---|---|
| `boa_engine` | JS interpreter |
| `serde_json` | Serialize/deserialize JS ↔ Rust values |
| `reqwest` (features = `["rustls-tls"]`) | OpenRouter API calls (subAgent + final stream) — must use `rustls-tls` to avoid OpenSSL system dependency |
| `tokio` | Async runtime |
| `super-ragondin-rag` | RagStore, RagConfig, searcher, embedder |

### Async bridging

Boa is a synchronous JS engine. Async Rust functions (LanceDB queries, OpenRouter API calls) are bridged using `tokio::task::block_in_place` + `Handle::current().block_on(...)`.

**Runtime requirement:** `block_in_place` requires a `multi_thread` Tokio runtime. The CLI already uses `tokio::runtime::Runtime::new()` (multi-thread by default). Tests that exercise sandbox functions must use `#[tokio::test(flavor = "multi_thread")]`.

### `listFiles` de-duplication

The LanceDB `chunks` table stores one row per chunk. `list_docs()` performs de-duplication client-side: fetch all rows matching the filter, de-duplicate by `doc_id` keeping the row with the maximum `mtime`, sort, then apply `limit`. This is correct for the expected data sizes (thousands of documents). The `limit` is applied post-deduplication, so it reflects unique document count.

## Testing

- Unit tests for each sandbox function using a mock `RagStore` trait
- Unit test for the tool-use loop with a mock OpenRouter client (verifies iteration limit, error propagation, message history accumulation)
- Tests that exercise async bridging use `#[tokio::test(flavor = "multi_thread")]`
- Integration test for a full `ask` query against a real LanceDB instance (ignored by default, requires `OPENROUTER_API_KEY`)
