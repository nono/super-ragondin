# Design: `saveFile` and `listDirs` JS tools for code-mode

**Date:** 2026-03-13
**Crate:** `crates/codemode/`

## Overview

Add two new JavaScript global functions to the code-mode sandbox: `saveFile` and `listDirs`. Together they allow the LLM agent to write files into the local sync directory and discover the existing directory structure before deciding where to place them.

Files written by `saveFile` are picked up by the normal super-ragondin sync flow, which propagates them to remote Cozy Cloud and updates the RAG store.

## JS API

```js
saveFile(path, content, options?)
// path:    string — relative to sync_dir (e.g. "notes/summary.md")
// content: string — file content
// options: { encoding: "utf8" | "base64" }  — default "utf8"
// returns: null (JS undefined serializes to null via JSON.stringify)
// throws:  if path escapes sync_dir, write fails, or base64 is invalid

listDirs(prefix?)
// prefix:  string — relative path within sync_dir (default: "")
// returns: string[] — direct child directory names at that level only (non-recursive)
//          e.g. listDirs()        → ["notes", "work", "photos"]
//               listDirs("work")  → ["projects", "meetings"]
```

`saveFile` creates intermediate directories automatically (equivalent to `mkdir -p`). `listDirs` with a non-existent prefix returns an empty array.

Note: `saveFile` returns `JsValue::undefined()` at the Rust level, but since the sandbox serializes the result via `JSON.stringify`, the LLM receives `"null"` as the tool result string for a successful call.

## New dependency

Add `base64` to `crates/codemode/Cargo.toml` via:

```
cargo add base64 -p super-ragondin-codemode
```

This is needed for decoding base64-encoded binary content in `saveFile`.

## Architecture

### SandboxContext and Sandbox

Add `sync_dir: PathBuf` to both `SandboxContext` and `Sandbox`.

`Sandbox::new` is currently `const fn` — this must be changed to a plain `fn` since `PathBuf` is heap-allocated. The new complete signature is:

```rust
pub fn new(store: Arc<RagStore>, config: RagConfig, sync_dir: PathBuf) -> Self
```

The `embedder` is not a constructor parameter — it is constructed inside `Sandbox::execute()` from `self.config`, unchanged from the current implementation.

The `sync_dir` flows as follows:
1. `CodeModeEngine` holds `sync_dir: PathBuf`
2. On each `spawn_blocking` call, `sync_dir` is cloned alongside `store_clone` and `config_clone` into the closure: `let sync_dir_clone = self.sync_dir.clone();`
3. Inside the closure, `Sandbox::new(store_clone, config_clone, sync_dir_clone)` is called
4. `Sandbox::execute()` copies `sync_dir` from `self` into the freshly created `SandboxContext` (alongside `store`, `embedder`, `config`, `handle`)
5. Tool functions read `sync_dir` from `SANDBOX_CTX` via `SANDBOX_CTX.with(...)`

`CodeModeEngine::new` gains a second parameter. The new signature is:

```rust
pub async fn new(config: RagConfig, sync_dir: PathBuf) -> Result<Self>
```

The CLI call site is `cmd_ask` in `crates/cli/src/main.rs`. It already loads the sync `Config` struct (which has a `sync_dir: PathBuf` field) before constructing `RagConfig`. The call becomes:

```rust
let engine = CodeModeEngine::new(rag_config, config.sync_dir).await?;
```

`sync_dir` lives on the sync `Config` struct, not on `RagConfig` — they are passed separately.

### Concurrent writes

`std::fs::create_dir_all` is safe under concurrent races: if two `saveFile` calls create the same parent directory simultaneously, `create_dir_all` silently ignores `AlreadyExists` errors. No additional synchronization is needed.

### New files

- `crates/codemode/src/tools/save_file.rs` — registers `saveFile` global
- `crates/codemode/src/tools/list_dirs.rs` — registers `listDirs` global

Both files follow the existing pattern: `pub fn register(ctx: &mut Context) -> Result<(), JsError>`. The `#[allow(dead_code)]` attribute is applied on `register` to suppress the clippy nursery lint for `pub` functions that are only called from within the crate, consistent with existing tool files.

### Modified files

- `crates/codemode/src/sandbox.rs` — add `sync_dir` to `SandboxContext` and `Sandbox`; change `Sandbox::new` from `const fn` to `fn`; update `make_sandbox()` test helper to create a `tempdir`-based `sync_dir` and pass it as the third argument
- `crates/codemode/src/tools/mod.rs` — add `pub mod save_file` and `pub mod list_dirs`
- `crates/codemode/src/engine.rs` — update `CodeModeEngine::new()` signature, clone `sync_dir` in `spawn_blocking` loop, register new tools, update `execute_js` tool description (see below)
- `crates/codemode/src/prompt.rs` — document `saveFile` and `listDirs` with examples (`system_prompt()` remains `const fn` since the content is still a `&'static str` literal)
- `crates/cli/src/main.rs` — pass `config.sync_dir` as second argument to `CodeModeEngine::new()`
- `AGENTS.md` — update the `crates/codemode/` section to document `save_file.rs` and `list_dirs.rs`

### `execute_js` tool description update

Replace the current description in `engine.rs`:

```
"Use the search(), listFiles(), getDocument(), and subAgent() functions to query the document database."
```

with:

```
"Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), and listDirs() functions to query the document database and write files."
```

## Implementation details

### Path traversal prevention

Both `saveFile` and `listDirs` use the same component-walk approach to prevent path traversal without touching the filesystem:

1. Iterate over the `std::path::Path::components()` of the user-supplied relative path.
2. Reject the path if any component is `Component::ParentDir` (`..`) or `Component::RootDir` (`/`).
3. `Component::CurDir` (`.`) components are silently skipped — they carry no meaning in a relative path and have no effect on the final joined path.
4. This normalizes the path purely in memory, eliminating any TOCTOU window.

This is simpler and safer than attempting to canonicalize an ancestor directory on disk.

### `save_file.rs`

1. Add `#[allow(dead_code)]` on `pub fn register`, following the pattern of existing tools.
2. Extract `path` (string) and `content` (string) from JS args; `options.encoding` defaults to `"utf8"`.
3. Reject absolute paths immediately (`"path must be relative"`).
4. Run the component-walk check on `path`; return `"path escapes sync directory"` if any `..` or root component is found.
5. Join `sync_dir` + `path` to get the target path.
6. Create parent directories with `std::fs::create_dir_all`.
7. If encoding is `"base64"`: decode `content` with the `base64` crate (engine `STANDARD`); return `"invalid base64 content"` on failure. If encoding is `"utf8"` (explicit or default): use the string bytes directly.
8. Write bytes with `std::fs::write`.
9. Return `JsValue::undefined()`.

### `list_dirs.rs`

1. Add `#[allow(dead_code)]` on `pub fn register`, following the pattern of existing tools.
2. Extract optional `prefix` string from JS args; default to `""`.
3. If prefix is non-empty, run the component-walk check; return `"path escapes sync directory"` if any `..` or root component is found.
4. Build target path: `sync_dir / prefix`.
5. If path does not exist, return empty JS array.
6. If path exists but is not a directory, return JS error `"not a directory"`.
7. Read directory entries with `std::fs::read_dir`, filter for entries where `entry.file_type().is_dir()`, collect names as strings, sort using Rust's default `str` ordering (lexicographic byte order, case-sensitive), return as JS array.

## Error handling

| Situation | Behavior |
|---|---|
| Path is absolute | JS `Error`: `"path must be relative"` |
| Path contains `..` or root component | JS `Error`: `"path escapes sync directory"` |
| Base64 decode failure | JS `Error`: `"invalid base64 content"` |
| Filesystem write error | JS `Error` wrapping the OS error message |
| `listDirs` prefix doesn't exist | Returns `[]` |
| `listDirs` prefix is a file | JS `Error`: `"not a directory"` |

## Testing

All tests are in `#[cfg(test)]` modules within their respective source files. `tempfile` is already in `crates/codemode/Cargo.toml` `[dev-dependencies]` and requires no new addition.

### `save_file.rs` unit tests

- Write a UTF-8 text file (default encoding) and verify content on disk
- Write a UTF-8 text file with `encoding: "utf8"` set explicitly and verify content on disk
- Write a base64-encoded binary file and verify decoded bytes on disk
- Auto-create intermediate parent directories
- Reject `../` path traversal
- Reject absolute paths
- Reject invalid base64 content

### `list_dirs.rs` unit tests

- Empty sync dir returns `[]`
- Returns only directory names, not files
- Nested prefix works correctly
- Non-existent prefix returns `[]`
- File path as prefix returns error
- `../` prefix returns path traversal error

### Existing test updates

- `sandbox.rs` — update `make_sandbox()` helper to create a `tempdir`-based `sync_dir` and pass it to `Sandbox::new`; extend `test_sandbox_globals_registered` to include `"saveFile"` and `"listDirs"`
- `prompt.rs` — extend `test_prompt_contains_key_elements` to assert `"saveFile("` and `"listDirs("` are present

## System prompt additions

Add to the "Available JavaScript functions" section:

```
  saveFile(path, content, options?)
    Write a file into the sync directory. options: { encoding: "utf8" | "base64" }
    Default encoding is "utf8". Use "base64" for binary content.
    Creates intermediate directories automatically.
    Returns: null

  listDirs(prefix?)
    Non-recursive: list only immediate subdirectory names at a given path within the sync directory.
    Returns: string[] — directory names only, sorted alphabetically
```

Add examples:

```js
// Discover top-level directories
listDirs()

// Explore a subdirectory before saving
const dirs = listDirs("work");
// dirs might be ["meetings", "projects"]

// Save a text summary
saveFile("notes/summary.md", "# Summary\n\nKey points...", { encoding: "utf8" })

// Save a generated image (base64)
saveFile("images/chart.png", base64EncodedPngString, { encoding: "base64" })
```
