# tauri-specta Type-Safe Bindings

**Date:** 2026-03-20
**Status:** Approved

## Goal

Eliminate the fragile string-literal contract between Rust enums and the Svelte frontend by generating TypeScript bindings from Rust types via `tauri-specta`. Migrate the frontend to TypeScript with Vite 8.

## Motivation

The current Rust→frontend contract is maintained by comments and convention: `AppState` serializes as `"Unconfigured"` etc., matched verbatim in `App.svelte`. Renaming a Rust variant silently breaks the UI with no compile-time error. The same risk exists for `SyncState`, `SyncStatus`, and `AuthError`.

## Scope

### In scope
- Add `tauri-specta` + `specta-typescript` to `crates/gui`
- Derive `specta::Type` on all public IPC types
- Define typed event structs with `tauri_specta::Event` derives
- Replace `tauri::generate_handler!` with the tauri-specta builder in `main.rs`
- Add an ignored export test to regenerate `bindings.ts`
- Upgrade frontend to Vite 8
- Add TypeScript via `typescript` + `svelte-check`
- Migrate all Svelte components to `lang="ts"` using typed bindings

### Out of scope
- Changes to sync logic, auth flow, or any non-GUI crate
- Adding new commands or events

### Note on TypeScript compiler

`@typescript/native-preview` (the Go-based `tsgo`) is not yet compatible with `svelte-check`'s peer dependency on the standard `typescript` npm package. We use `typescript` for `svelte-check` IDE integration. Vite 8 already uses esbuild for transpilation, so build speed is unaffected.

## Rust Changes (`crates/gui/`)

### Dependencies

Add to `Cargo.toml`:
```toml
specta-typescript = "0.0.9"
tauri-specta = { version = "2", features = ["derive", "typescript"] }
```

### Type annotations

Derive `specta::Type` on the four types that cross the IPC boundary:

- `AppState` — return type of `get_app_state`
- `AuthError` — payload of `auth_error` event
- `SyncState` — embedded in `SyncStatus`
- `SyncStatus` — payload of `sync_status` event

Example:
```rust
#[derive(Debug, serde::Serialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "PascalCase")]
pub enum AppState {
    Unconfigured,
    Unauthenticated,
    Ready,
}
```

### Event structs

Each emitted event needs a dedicated struct with `tauri_specta::Event` and `specta::Type` derives. Three new event types are added in `commands.rs`:

```rust
/// Emitted when OAuth completes successfully.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AuthCompleteEvent;

/// Emitted when OAuth fails.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct AuthErrorEvent {
    pub message: String,
}

/// Emitted each sync loop iteration.
#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct SyncStatusEvent {
    pub status: SyncState,
    pub last_sync: Option<String>,
}
```

The existing `AuthError` struct is replaced by `AuthErrorEvent`. The existing `SyncStatus` struct is replaced by `SyncStatusEvent`. All `app.emit(...)` call sites are updated to use the new typed event structs.

### Shared builder helper

To share the builder between `main()` and the export test without duplication, extract a `make_builder()` function:

```rust
pub fn make_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            get_app_state,
            init_config,
            start_auth,
            start_sync,
        ])
        .events(tauri_specta::collect_events![
            AuthCompleteEvent,
            AuthErrorEvent,
            SyncStatusEvent,
        ])
}
```

### `main.rs` — builder swap

Replace `tauri::generate_handler!` with the tauri-specta builder. Call `invoke_handler()` first, then move the builder into the `.setup()` closure to call `mount_events()`:

```rust
let builder = commands::make_builder();

tauri::Builder::default()
    .manage(SyncGuard::default())
    .invoke_handler(builder.invoke_handler())
    .setup(move |app| {
        builder.mount_events(app);
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

The `mount_events` call in `.setup()` is mandatory — without it, typed event emission will panic at runtime. The `move` on the closure is required because `builder` must be owned by the closure.

### Export test

An ignored test in `commands.rs` writes `bindings.ts` to the frontend source tree:

```rust
#[test]
#[ignore]
fn export_bindings() {
    make_builder()
        .export(
            specta_typescript::Typescript::default(),
            "../gui-frontend/src/bindings.ts",
        )
        .expect("Failed to export bindings");
}
```

**Regeneration command:** `cargo test export_bindings -- --ignored`

Run this after any change to a type or command signature, then:
1. `cargo fmt --all`
2. `cargo clippy --all-features`
3. Commit the updated `bindings.ts` alongside the Rust change

## Frontend Changes (`gui-frontend/`)

### Vite upgrade

Update `vite` from `^5` to `^8` in `package.json`.

### TypeScript toolchain

Add to `devDependencies`:
- `typescript` — TypeScript compiler (required by `svelte-check`)
- `svelte-check` — Svelte-aware type checking
- `@tsconfig/svelte` — base tsconfig for Svelte projects

Add `tsconfig.json`:
```json
{
  "extends": "@tsconfig/svelte/tsconfig.json",
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true
  },
  "include": ["src/**/*"]
}
```

### Generated `bindings.ts`

Committed at `gui-frontend/src/bindings.ts`. Exports:
- Types: `AppState`, `SyncState`, `SyncStatusEvent`, `AuthErrorEvent`
- Typed command wrappers: `commands.getAppState()`, `commands.initConfig(...)`, `commands.startAuth()`, `commands.startSync()`
- Typed event listeners: `events.authCompleteEvent.listen(...)`, `events.authErrorEvent.listen(...)`, `events.syncStatusEvent.listen(...)`

Note: tauri-specta generates camelCase accessor keys from the struct name, e.g. `AuthCompleteEvent` → `events.authCompleteEvent`.

### Svelte component migration

All four components get `lang="ts"` on their `<script>` tag and replace raw `invoke`/`listen` calls with typed wrappers from `bindings.ts`.

**`App.svelte`:**
```typescript
import { commands, events } from './bindings'
import type { AppState } from './bindings'

let appState: AppState | null = null

appState = await commands.getAppState()
events.authCompleteEvent.listen(() => { appState = 'Ready'; authError = null })
events.authErrorEvent.listen((e) => { authError = e.payload.message })
```

**`Syncing.svelte`:**
```typescript
import { commands, events } from './bindings'
import type { SyncState } from './bindings'

let status: SyncState = 'Idle'
events.syncStatusEvent.listen((e) => {
  status = e.payload.status
  lastSync = e.payload.last_sync
})
commands.startSync()
```

**`Setup.svelte`:** `invoke('init_config', {...})` → `commands.initConfig(instanceUrl, syncDir)`

**`Auth.svelte`:** `invoke('start_auth')` → `commands.startAuth()`

## Testing

- Existing Rust unit tests in `commands.rs` are unaffected
- `cargo fmt --all` and `cargo clippy --all-features` must pass after Rust changes
- `cargo test export_bindings -- --ignored` must produce a valid `bindings.ts`
- Frontend type checking via `svelte-check` must pass with zero errors
