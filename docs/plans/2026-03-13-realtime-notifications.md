# Realtime Notifications

**Date:** 2026-03-13
**Status:** Accepted

## Goal

Replace the 30-second polling delay for remote changes with WebSocket-based
realtime notifications from the Cozy stack. When a file is created, updated, or
deleted on the remote, the client should trigger a sync cycle within ~200ms
instead of waiting up to 30 seconds.

The realtime WebSocket is used **only as a trigger** — event payloads are
ignored. The actual changes are fetched via the existing `fetch_changes` polling
endpoint (which provides `last_seq` ordering guarantees).

## Architecture

### New module: `crates/sync/src/remote/realtime.rs`

A `RealtimeListener` struct that manages the WebSocket connection lifecycle.

```rust
pub struct RealtimeListener { /* ... */ }

impl RealtimeListener {
    pub fn new(instance_url: &str, access_token: &str) -> Self;

    /// Spawns the WebSocket listener as a tokio task.
    /// Returns a receiver that gets () on each remote change (debounced).
    pub fn start(self, cancel: CancellationToken) -> mpsc::Receiver<()>;
}
```

### WebSocket protocol

The Cozy stack exposes a WebSocket at `/realtime/` using the
`io.cozy.websocket` protocol:

1. **Connect** to `wss://<instance>/realtime/` (or `ws://` for localhost).
2. **Authenticate:** `{"method": "AUTH", "payload": "<access_token>"}`.
3. **Subscribe:** `{"method": "SUBSCRIBE", "payload": {"type": "io.cozy.files"}}`.
4. **Listen** for `CREATED`, `UPDATED`, `DELETED` events.
5. On any event, send a debounced `()` nudge on the channel.

### Debouncing

Events are debounced with:
- **200ms** quiet period (reset on each new event).
- **5s** max wait (force a nudge even if events keep arriving).

Implemented with `tokio::time::sleep` — no external debounce library.

### Reconnection

- On disconnect or error: log warning, wait **10 seconds**, reconnect.
- On auth/permission failure (403): log error, **stop retrying**.
- On malformed messages: log debug, ignore, keep listening.
- The listener **never causes a fatal error** in the sync loop.

### Integration with `cmd_watch`

The existing synchronous watch loop is preserved. A bridging thread forwards
realtime nudges into the existing `std::sync::mpsc` channel:

```
Local inotify event  →  sync cycle (2s debounce, unchanged)
Realtime nudge       →  sync cycle (new WatchEvent::Remote variant)
Neither for 30s      →  sync cycle (safety net, kept as-is)
```

Changes to `crates/cli/src/main.rs`:
- Add `WatchEvent::Remote` variant.
- Spawn a bridge thread that blocks on the async realtime receiver and sends
  `WatchEvent::Remote` on the existing `tx`.
- The loop logic stays the same — any event triggers a sync cycle.

The 30-second periodic poll is **kept** as a safety net in case the WebSocket
disconnects silently.

## Dependencies

Add `tokio-tungstenite` with `rustls-tls-webpki-roots` feature to
`crates/sync/Cargo.toml`:

```
cargo add tokio-tungstenite --features rustls-tls-webpki-roots -p super-ragondin-sync
```

No other new dependencies. `serde_json`, `tokio`, and `tracing` are already
present.

## Error handling

| Scenario | Behavior |
|---|---|
| Connection failure | Log warn, retry after 10s |
| Auth rejection (403) | Log error, stop retrying |
| Malformed message | Log debug, ignore |
| WebSocket close frame | Treat as disconnect, retry after 10s |
| Channel closed (receiver dropped) | Task exits silently |
| Cancel token triggered | Task exits silently |

## Testing

### Unit tests

- Message serialization (AUTH, SUBSCRIBE JSON format).
- Debounce logic: multiple rapid events produce one nudge; max-wait forces a
  nudge after 5s.

### Integration tests (mock WebSocket server)

- Connect → AUTH → SUBSCRIBE handshake sequence.
- Server sends CREATED event → receiver gets `()`.
- Server closes connection → client reconnects after delay.
- Server sends 403 → client stops retrying.

Uses `tokio-tungstenite` to spin up a local WebSocket server in-test. No real
cozy-stack required.

## Implementation steps

1. Add `tokio-tungstenite` dependency.
2. Create `crates/sync/src/remote/realtime.rs` with `RealtimeListener`.
3. Export from `crates/sync/src/remote/mod.rs` (or `remote.rs`).
4. Add unit tests for message format and debounce logic.
5. Add integration tests with mock WebSocket server.
6. Add `WatchEvent::Remote` variant in CLI.
7. Wire up `RealtimeListener` in `cmd_watch` with bridge thread.
8. Run `cargo fmt --all && cargo clippy --all-features`.

## References

- [Cozy stack realtime docs](https://docs.cozy.io/en/cozy-stack/realtime/)
- Old client: `core/remote/watcher/realtime_manager.js`
- Old client: `core/remote/watcher/index.js` (integration with remote watcher)
