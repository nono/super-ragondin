# Tauri GUI Design

**Date:** 2026-03-19
**Status:** Approved

## Overview

Add a Tauri v2 desktop GUI to Super Ragondin. The CLI (`crates/cli`) is kept unchanged. A new `crates/gui` workspace member provides the Tauri binary with a Svelte frontend in `gui-frontend/`. The two binaries share all existing libraries (`sync`, `cozy-client`, `rag`, `codemode`) and the same config file on disk.

## Screens

Three screens, driven by app state:

1. **Setup** (`Unconfigured`): form with Cozy instance URL + sync directory picker → "Connect to Cozy" button
2. **Auth** (`Unauthenticated`): shown while OAuth is in progress — "Authorize in your browser" message with spinner; auto-starts OAuth on mount
3. **Syncing** (`Ready`): "Synchronizing" status with sync directory path and last-sync timestamp

On launch, the app reads the config and jumps directly to the appropriate screen. A user who already configured via CLI sees Screen 3 immediately.

## Project Structure

```
Cargo.toml                # workspace — add "crates/gui" to members array

crates/gui/               # NEW — Tauri binary (Rust backend)
  src/main.rs             # Tauri builder setup
  src/commands.rs         # Tauri commands + sync loop
  tauri.conf.json         # frontendDist: "../../gui-frontend/dist"
                          # devUrl: "http://localhost:5173" (for `tauri dev`)
  Cargo.toml

gui-frontend/             # NEW — Svelte frontend (at workspace root)
  src/App.svelte          # State machine: routes to correct screen
  src/lib/Setup.svelte
  src/lib/Auth.svelte
  src/lib/Syncing.svelte
  package.json
  vite.config.js
```

All existing crates are unchanged.

## Tauri Commands

| Command | Input | Output / side-effects |
|---|---|---|
| `get_app_state` | — | Returns `"Unconfigured"`, `"Unauthenticated"`, or `"Ready"` |
| `init_config` | `{ instance_url: string, sync_dir: string }` | Creates `sync_dir`, `data_dir`, `data_dir/staging`; writes `config.json`; returns `Ok` or error string |
| `start_auth` | — | Registers OAuth client, opens browser, spins up `:8080` server, exchanges code, saves config; emits `auth_complete` or `auth_error`; returns immediately |
| `start_sync` | — | No-op if already running; otherwise spawns watch loop in background; returns `Ok` |

### `init_config` — directory layout

`data_dir` is determined the same way as the CLI: `dirs::data_dir()/super-ragondin`. The command creates:
- `sync_dir` (user-provided)
- `data_dir`
- `data_dir/staging`

### `start_auth` — detailed flow

1. Call `OAuthClient::register(instance_url, "Super Ragondin", "super-ragondin")` (from `super_ragondin_sync::remote::auth`, which re-exports from `super-ragondin-cozy-client`)
2. Generate a random UUID `state`
3. Build auth URL with `oauth.authorization_url(&state)`
4. Open URL in system browser via the `opener` crate
5. Bind `TcpListener` on `127.0.0.1:8080` — loopback only, never `0.0.0.0`. If bind fails (port in use), emit `auth_error { message: "Port 8080 is already in use. Close other applications and try again." }` and return
6. Accept one connection; read the HTTP request line; extract `code` and `state` query parameters
7. Validate that the received `state` matches the generated UUID. If it doesn't match, emit `auth_error { message: "OAuth state mismatch — possible CSRF attempt." }` and return
8. Send a plain HTTP `200 OK` response with a brief success message, then close the listener
9. Call `oauth.exchange_code(code)` → save config
10. Emit `auth_complete {}` on success, or `auth_error { message: "..." }` on any error

### `start_sync` — idempotency and concurrency

The command is guarded by a `Mutex<bool>` in Tauri state: if `true` (already running), return immediately. Otherwise set to `true` and spawn the watch loop.

The background loop runs via `tokio::task::spawn` on Tauri's existing async runtime (Tauri v2 is async-first). Do **not** create a new `tokio::runtime::Runtime` — that would be redundant and wasteful inside a Tauri process.

`start_sync` reads the config fresh from disk (`Config::load(&config_path())`) at invocation time, ensuring it uses the OAuth tokens written by `start_auth` rather than any stale in-memory copy.

The loop emits `sync_status` events. There is no `stop_sync` command — the loop runs until the process exits.

## OAuth Types

`OAuthClient` is used from `super_ragondin_sync::remote::auth` (which re-exports `super_ragondin_cozy_client::auth`). `crates/gui` depends on `super-ragondin-sync`; it does not need to depend on `super-ragondin-cozy-client` directly.

## Events (Rust → Frontend)

| Event | Payload | Description |
|---|---|---|
| `auth_complete` | `{}` | OAuth succeeded; frontend transitions to Syncing screen |
| `auth_error` | `{ message: string }` | OAuth failed; frontend shows the message and stays on Auth screen |
| `sync_status` | `{ status: "syncing" \| "idle", last_sync: string \| null }` | Emitted after each sync cycle; `last_sync` is an ISO 8601 UTC string (e.g. `"2026-03-19T14:23:00Z"`) or `null` if no sync has completed yet |

## Frontend ↔ Backend Communication

- **Frontend → Rust:** `invoke("command_name", { args })` via `@tauri-apps/api/core`
- **Rust → Frontend:** `app_handle.emit("event_name", payload)` via Tauri's event system
- **Frontend listens** with `listen("event_name", handler)` from `@tauri-apps/api/event`

### `App.svelte` state model

`App.svelte` holds a single Svelte `writable` store (or reactive `let` variable) `appState: "Unconfigured" | "Unauthenticated" | "Ready"`.

`get_app_state` returns `"Ready"` only when `config.oauth_client` is `Some` **and** `oauth_client.access_token` is `Some`. A config with a registered-but-unauthenticated client (e.g. from an incomplete previous auth) is treated as `"Unauthenticated"`.

On mount:
1. Call `invoke("get_app_state")` → set `appState`
2. Register `listen("auth_complete", () => appState = "Ready")`
3. Register `listen("auth_error", (e) => authError = e.payload.message)`

`App.svelte` renders `<Setup>`, `<Auth>`, or `<Syncing>` based on `appState`. When Setup submits, it calls `invoke("init_config", ...)` then sets `appState = "Unauthenticated"`. `Auth.svelte` calls `invoke("start_auth")` on mount.

`Auth.svelte` displays an inline error message below the spinner when `authError` is set, along with a "Retry" button that calls `invoke("start_auth")` again. The Auth screen does not auto-retry on failure.

## Shared Config

Both binaries use the same config path: `config_dir()/super-ragondin/config.json`. A user configured via CLI gets a working GUI immediately.

## Tauri Configuration

**`crates/gui/Cargo.toml`** — key dependencies:
- `tauri = { version = "2", features = ["protocol-asset"] }`
- `tauri-build` (build dependency)
- `opener` — open system browser
- `tokio` (with `full` features)
- `super-ragondin-sync` (workspace)
- `uuid` with `v4` feature (generate OAuth state)
- `serde`, `serde_json`

**`tauri.conf.json`** — key settings:
- `build.frontendDist`: `"../../gui-frontend/dist"`
- `build.devUrl`: `"http://localhost:5173"`
- Capabilities must explicitly allow: `get_app_state`, `init_config`, `start_auth`, `start_sync`

## Window

Fixed size ~420×320px. No system tray. Closing the window stops the app (and sync). UX improvements deferred.

## Out of Scope

- System tray / background running after window close
- Sync error display / conflict notifications
- Settings screen (change URL or directory)
- RAG / `ask` functionality in the GUI
- Packaging / distribution
- Stopping the sync loop without quitting the app
