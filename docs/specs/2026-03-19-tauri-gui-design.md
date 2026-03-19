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
crates/gui/               # NEW — Tauri binary (Rust backend)
  src/main.rs             # Tauri builder setup
  src/commands.rs         # Tauri commands + sync loop
  tauri.conf.json
  Cargo.toml

gui-frontend/             # NEW — Svelte frontend
  src/App.svelte          # State machine: routes to correct screen
  src/lib/Setup.svelte
  src/lib/Auth.svelte
  src/lib/Syncing.svelte
  package.json
  vite.config.js
```

All existing crates are unchanged.

## Tauri Commands

| Command | Description |
|---|---|
| `get_app_state` | Reads config from disk; returns `Unconfigured`, `Unauthenticated`, or `Ready` |
| `init_config(instance_url, sync_dir)` | Creates directories, writes `config.json` |
| `start_auth` | Registers OAuth client, opens system browser, spins up `:8080` callback server, exchanges code, saves config; emits `auth_complete` on success |
| `start_sync` | Spawns watch loop (local watcher + realtime WebSocket + periodic sync) in background; emits `sync_status` events |

## OAuth Flow

The existing `REDIRECT_URI = "http://localhost:8080/callback"` is reused unchanged. `start_auth` opens the Cozy authorization URL in the system browser via the `opener` crate, then runs a `tokio::net::TcpListener` on port 8080. When the callback arrives (`GET /callback?code=...`), it extracts the code, calls `oauth.exchange_code(code)`, saves the config, and emits `auth_complete` to the frontend.

## Sync Loop

The watch logic from `cmd_watch` (CLI) is re-implemented in `crates/gui/src/commands.rs` for the `start_sync` command. It runs in `tauri::async_runtime::spawn` and emits `sync_status` events (e.g. `{ status: "syncing" | "idle", last_sync: "..." }`) to the frontend instead of logging to tracing.

## Shared Config

Both binaries use the same config path: `config_dir()/super-ragondin/config.json`. A user configured via CLI gets a working GUI immediately.

## Frontend ↔ Backend Communication

- **Frontend → Rust:** `invoke("command_name", { args })` via `@tauri-apps/api/core`
- **Rust → Frontend:** `app_handle.emit("event_name", payload)` via Tauri's event system
- Frontend listens with `listen("event_name", handler)` from `@tauri-apps/api/event`

## Window

Fixed size ~420×320px. No system tray. Closing the window stops the app (and sync). UX improvements deferred.

## Dependencies

**`crates/gui/Cargo.toml`** additions:
- `tauri = { version = "2", features = ["..."] }`
- `tauri-build` (build dependency)
- `opener` — open system browser
- `tokio` — already in workspace
- Existing workspace crates: `super-ragondin-sync`, `super-ragondin-cozy-client`

**`gui-frontend/package.json`** additions:
- `@tauri-apps/api`
- `@tauri-apps/cli` (dev)
- `svelte`, `vite`, `@sveltejs/vite-plugin-svelte` (dev)

## Out of Scope

- System tray / background running after window close
- Sync error display / conflict notifications
- Settings screen (change URL or directory)
- RAG / `ask` functionality in the GUI
- Packaging / distribution
