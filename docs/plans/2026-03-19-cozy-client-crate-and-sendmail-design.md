# Extract Cozy client crate & add sendMail tool

## Goal

1. Extract the Cozy HTTP client, OAuth auth, and realtime listener into a dedicated `crates/cozy-client/` crate (`super-ragondin-cozy-client`)
2. Add a `send_mail()` method to `CozyClient` using the `/jobs/queue/sendmail` endpoint (mode `noreply`)
3. Expose a `sendMail(subject, body)` JS global in the codemode sandbox

## New crate: `crates/cozy-client/`

### Structure

```
crates/cozy-client/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── auth.rs        — OAuthClient (register, exchange_code, refresh)
    ├── client.rs      — CozyClient (fetch_changes, download, upload, send_mail, …)
    ├── error.rs       — Error enum (Http, Json, Url, InvalidTimestamp, InvalidMd5, MissingDocument)
    ├── realtime.rs    — RealtimeListener (WebSocket)
    └── types.rs       — RemoteId, NodeType, RemoteNode, MailPart
```

### Types moved from `sync/model.rs`

- `RemoteId` — remote Cozy document ID
- `NodeType` — `File` | `Directory`
- `RemoteNode` — remote node with id, parent_id, name, type, md5, size, rev

These are re-used by `sync` via its dependency on `cozy-client`.

### Types staying in `sync/model.rs`

- `LocalFileId`, `LocalNode` — local filesystem types
- `SyncedRecord` — binding between local and remote
- `SyncOp`, `Conflict`, `PlanResult` — sync planning types
- `NodeInfo` trait — stays in sync (uses both local and remote nodes)

### Error type

Minimal error enum covering HTTP/API concerns only:

- `Http(reqwest::Error)`
- `Json(serde_json::Error)`
- `InvalidUrl(url::ParseError)`
- `InvalidTimestamp(String)`
- `InvalidMd5(String)`
- `MissingDocument(String)`
- `NotFound(String)`
- `RevisionMismatch { expected, actual }`

The `sync` crate's `Error` will have a `Cozy(cozy_client::Error)` variant wrapping these.

### Dependencies

- `reqwest` (with `rustls-tls`)
- `serde`, `serde_json`
- `thiserror`
- `tracing`
- `chrono`
- `base64`, `hex`
- `urlencoding`
- `tokio-tungstenite` (for realtime)
- `tokio`, `tokio-util`, `futures` (async runtime)

### `util::deserialize_string_or_u64`

This helper is used by both `client.rs` (raw deserialization) and `model.rs` (`RemoteNode`, `SyncedRecord`). It moves to `cozy-client` (in `types.rs` or a small `util.rs`), and `sync` either re-imports it or keeps a copy for `SyncedRecord`.

## `send_mail()` method

```rust
impl CozyClient {
    pub async fn send_mail(&self, subject: &str, parts: &[MailPart]) -> Result<()> {
        // POST /jobs/queue/sendmail
        // mode: "noreply" (hardcoded)
        // subject + parts from arguments
    }
}
```

### `MailPart`

```rust
pub enum MailContentType {
    TextPlain,
    TextHtml,
}

pub struct MailPart {
    pub content_type: MailContentType,
    pub body: String,
}

impl MailPart {
    pub fn plain(body: impl Into<String>) -> Self { … }
    pub fn html(body: impl Into<String>) -> Self { … }
}
```

### Required permission

The OAuth client must have `io.cozy.jobs:POST:sendmail:worker`. This is a registration-time concern — handled separately from this refactoring.

## JS sandbox tool: `sendMail(subject, body)`

### File: `crates/codemode/src/tools/send_mail.rs`

- Registers `sendMail(subject, body)` as a JS global
- `subject`: string — email subject line
- `body`: string — plain text body
- Returns `undefined` on success, throws JS error on failure

### Wiring

- `SandboxContext` gains: `pub cozy_client: Option<Arc<CozyClient>>`
- `Sandbox::new()` accepts an optional `Arc<CozyClient>`
- `CodeModeEngine::new()` accepts an optional `Arc<CozyClient>` (from CLI)
- If `cozy_client` is `None` and `sendMail()` is called, throws `"Cozy client not configured"`
- `execute_js_tool_definition()` description updated to mention `sendMail()`

## Implementation steps

1. **Create `crates/cozy-client/`** — new crate with `error.rs`, `types.rs` (`RemoteId`, `NodeType`, `RemoteNode`, `MailPart`, `deserialize_string_or_u64`)
2. **Move `client.rs`, `auth.rs`, `realtime.rs`** from `sync/src/remote/` into the new crate, update imports
3. **Update `sync`** — depend on `cozy-client`, replace moved types with re-imports, add `Cozy(cozy_client::Error)` variant to sync error
4. **Add `send_mail()` method** to `CozyClient` with a `wiremock` test
5. **Wire `CozyClient` into codemode** — add `Option<Arc<CozyClient>>` to `SandboxContext`, `Sandbox`, `CodeModeEngine`
6. **Add `send_mail.rs` tool** — register `sendMail(subject, body)` JS global
7. **Update CLI** — pass `CozyClient` to `CodeModeEngine` when running `ask`

Each step should leave the project compiling and tests green.
