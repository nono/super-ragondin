# tauri-specta Type-Safe Bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace string-literal Rust→frontend IPC contracts with generated TypeScript bindings via tauri-specta, and migrate the Svelte frontend to TypeScript with Vite 8.

**Architecture:** Add `tauri-specta` + `specta-typescript` to `crates/gui`, derive `specta::Type` and `tauri_specta::Event` on all IPC-crossing types, wire a shared `make_builder()` into both `main.rs` and a `#[test] #[ignore]` export test. On the frontend, upgrade Vite to v8, add TypeScript tooling, and migrate all four Svelte components to `lang="ts"` importing from the generated `bindings.ts`.

**Tech Stack:** Rust, Tauri v2, tauri-specta v2, specta-typescript 0.0.9, Svelte 4, Vite 8, TypeScript, svelte-check

---

## Files Changed

| File | Change |
|------|--------|
| `crates/gui/Cargo.toml` | Add `specta-typescript`, `tauri-specta` dependencies |
| `crates/gui/src/commands.rs` | Add `specta::Type` derives, add 3 event structs, replace `AuthError`/`SyncStatus` with typed events, add `make_builder()`, add export test |
| `crates/gui/src/main.rs` | Replace `generate_handler!` with tauri-specta builder |
| `gui-frontend/package.json` | Upgrade vite to ^8, add typescript/svelte-check/@tsconfig/svelte |
| `gui-frontend/tsconfig.json` | New file |
| `gui-frontend/src/bindings.ts` | Generated — committed after `cargo test export_bindings -- --ignored` |
| `gui-frontend/src/App.svelte` | `lang="ts"`, typed bindings |
| `gui-frontend/src/lib/Syncing.svelte` | `lang="ts"`, typed bindings |
| `gui-frontend/src/lib/Setup.svelte` | `lang="ts"`, typed bindings |
| `gui-frontend/src/lib/Auth.svelte` | `lang="ts"`, typed bindings |

---

## Task 1: Add Rust dependencies

**Files:**
- Modify: `crates/gui/Cargo.toml`

- [ ] **Step 1: Add dependencies via cargo add**

Run from the workspace root:
```bash
cargo add --package super-ragondin-gui specta-typescript@0.0.9
cargo add --package super-ragondin-gui tauri-specta@2 --features derive,typescript
```

- [ ] **Step 2: Verify the build compiles**

```bash
cargo build -p super-ragondin-gui
```

Expected: compiles without errors.

- [ ] **Step 3: Run existing tests to confirm nothing broke**

```bash
cargo test -q -p super-ragondin-gui
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/gui/Cargo.toml Cargo.lock
git commit -m "feat(gui): add tauri-specta and specta-typescript dependencies"
```

---

## Task 2: Add `specta::Type` derives and typed event structs

**Files:**
- Modify: `crates/gui/src/commands.rs`

Context: We need to:
1. Add `specta::Type` to 4 existing types
2. Replace `AuthError` with `AuthErrorEvent` (adds `Deserialize` + `tauri_specta::Event` derives)
3. Replace `SyncStatus` with `SyncStatusEvent` (same)
4. Add `AuthCompleteEvent` (unit struct, new)
5. Update all `app.emit(...)` call sites to use the new typed event API

The `tauri_specta::Event` trait (brought into scope with `use tauri_specta::EventExt;` if needed) provides an `.emit(manager)` instance method and a `::listen(manager, handler)` class method.

- [ ] **Step 1: Update `AppState` derive**

In `commands.rs` line 171, change:
```rust
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
```
to:
```rust
#[derive(Debug, serde::Serialize, PartialEq, Eq, specta::Type)]
```

- [ ] **Step 2: Update `SyncState` derive**

Find `SyncState` (~line 242), change:
```rust
#[derive(Debug, serde::Serialize, Clone, PartialEq, Eq)]
```
to:
```rust
#[derive(Debug, serde::Serialize, Clone, PartialEq, Eq, specta::Type)]
```

- [ ] **Step 3: Replace `AuthError` struct with typed event structs**

Remove the existing `AuthError` struct (lines 15-18):
```rust
#[derive(Debug, serde::Serialize, Clone)]
pub struct AuthError {
    pub message: String,
}
```

Add three new event structs in its place:
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

Also remove the old `SyncStatus` struct (~line 251-254):
```rust
#[derive(serde::Serialize, Clone)]
pub struct SyncStatus {
    pub status: SyncState,
    pub last_sync: Option<String>,
}
```

- [ ] **Step 4: Update `run_auth_flow` emit call sites**

There are multiple `app.emit("auth_error", AuthError { ... })` calls in `run_auth_flow`. Replace each with the typed API. For example:

```rust
// Before
app.emit("auth_error", AuthError { message: "...".to_string() })?;

// After
AuthErrorEvent { message: "...".to_string() }.emit(&app)?;
```

Apply to all 6 error emit calls in `run_auth_flow`. Also update the success emit:
```rust
// Before
app.emit("auth_complete", ())?;

// After
AuthCompleteEvent.emit(&app)?;
```

- [ ] **Step 5: Update `start_auth` error emit**

In `start_auth` (~line 157):
```rust
// Before
let _ = app.emit("auth_error", AuthError { message: e.to_string() });

// After
let _ = AuthErrorEvent { message: e.to_string() }.emit(&app);
```

- [ ] **Step 6: Update `run_sync_loop` emit call sites**

Two `app.emit("sync_status", SyncStatus { ... })` calls in `run_sync_loop`. Replace with:
```rust
// Before
let _ = app.emit("sync_status", SyncStatus { status: SyncState::Syncing, last_sync: last_sync.clone() });

// After
let _ = SyncStatusEvent { status: SyncState::Syncing, last_sync: last_sync.clone() }.emit(&app);
```

Apply to both the Syncing and Idle emit calls.

- [ ] **Step 7: Run tests to verify nothing broke**

```bash
cargo test -q -p super-ragondin-gui
```

Expected: all tests pass. The unit tests only test `parse_callback`, `app_state_from_config`, and `init_config_to` — none touch the emit call sites, so no test changes needed.

- [ ] **Step 8: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features
```

Expected: no warnings. If clippy warns about unused imports (e.g. old `use tauri::Emitter`), remove them.

- [ ] **Step 9: Commit**

```bash
git add crates/gui/src/commands.rs
git commit -m "feat(gui): add specta::Type derives and typed event structs"
```

---

## Task 3: Add `make_builder()`, update `main.rs`, generate `bindings.ts`

**Files:**
- Modify: `crates/gui/src/commands.rs`
- Modify: `crates/gui/src/main.rs`
- Create: `gui-frontend/src/bindings.ts` (generated)

- [ ] **Step 1: Add `make_builder()` and export test to `commands.rs`**

Add after the `SyncGuard` definition:

```rust
/// Build the tauri-specta builder — shared by `main()` and the export test.
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

Add at the end of the `#[cfg(test)]` block:

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

- [ ] **Step 2: Update `main.rs`**

Replace the current `tauri::Builder` setup:

```rust
// Before
fn main() {
    tauri::Builder::default()
        .manage(SyncGuard::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::init_config,
            commands::start_auth,
            commands::start_sync,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// After
fn main() {
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
}
```

- [ ] **Step 3: Run tests to confirm build and tests still pass**

```bash
cargo test -q -p super-ragondin-gui
```

Expected: all non-ignored tests pass.

- [ ] **Step 4: Format and lint**

```bash
cargo fmt --all
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 5: Generate `bindings.ts`**

Run from the workspace root:
```bash
cargo test -p super-ragondin-gui export_bindings -- --ignored
```

Expected: exits 0, creates `gui-frontend/src/bindings.ts`. Read the file to understand the exact exported names (command keys, event accessor names) — these will be used in the Svelte migration tasks.

- [ ] **Step 6: Commit**

```bash
git add crates/gui/src/commands.rs crates/gui/src/main.rs gui-frontend/src/bindings.ts
git commit -m "feat(gui): add tauri-specta builder, export test, and generated bindings"
```

---

## Task 4: Upgrade frontend to Vite 8 + TypeScript toolchain

**Files:**
- Modify: `gui-frontend/package.json`
- Create: `gui-frontend/tsconfig.json`

- [ ] **Step 1: Update `package.json`**

Change `gui-frontend/package.json` to:
```json
{
  "name": "gui-frontend",
  "private": true,
  "version": "0.0.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview",
    "check": "svelte-check --tsconfig ./tsconfig.json"
  },
  "dependencies": {
    "@tauri-apps/api": "^2"
  },
  "devDependencies": {
    "@sveltejs/vite-plugin-svelte": "^3",
    "@tsconfig/svelte": "^5",
    "svelte": "^4",
    "svelte-check": "^4",
    "typescript": "^5",
    "vite": "^8"
  }
}
```

- [ ] **Step 2: Create `tsconfig.json`**

Create `gui-frontend/tsconfig.json`:
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

- [ ] **Step 3: Install dependencies**

```bash
cd gui-frontend && npm install
```

Expected: installs Vite 8, typescript, svelte-check, @tsconfig/svelte. No peer dependency errors.

- [ ] **Step 4: Verify build still works**

```bash
npm run build
```

Expected: build succeeds. (Components are still plain JS at this point — no type errors yet.)

- [ ] **Step 5: Run svelte-check baseline**

```bash
npm run check
```

Expected: may report type errors in the still-JS components (acceptable at this stage — confirms the tool works).

- [ ] **Step 6: Commit**

```bash
cd ..
git add gui-frontend/package.json gui-frontend/package-lock.json gui-frontend/tsconfig.json
git commit -m "feat(gui): upgrade to Vite 8 and add TypeScript toolchain"
```

---

## Task 5: Migrate `App.svelte` to TypeScript

**Files:**
- Modify: `gui-frontend/src/App.svelte`

Before starting: read `gui-frontend/src/bindings.ts` to confirm exact exported names (e.g. `events.authCompleteEvent` vs `events.authComplete`).

- [ ] **Step 1: Add `lang="ts"` — red phase**

Change `<script>` to `<script lang="ts">` and run svelte-check:
```bash
cd gui-frontend && npm run check
```

Expected: type errors about untyped variables (`appState`, `authError`) and missing type imports.

- [ ] **Step 2: Replace imports and add types — green phase**

Replace the `<script lang="ts">` block in `App.svelte`:

```typescript
<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from './bindings'
  import type { AppState } from './bindings'
  import Setup from './lib/Setup.svelte'
  import Auth from './lib/Auth.svelte'
  import Syncing from './lib/Syncing.svelte'

  let appState: AppState | null = null
  let authError: string | null = null

  let unlistenAuthComplete: (() => void) | undefined
  let unlistenAuthError: (() => void) | undefined

  onMount(async () => {
    unlistenAuthComplete = await events.authCompleteEvent.listen(() => {
      appState = 'Ready'
      authError = null
    })
    unlistenAuthError = await events.authErrorEvent.listen((event) => {
      authError = event.payload.message
    })
    appState = await commands.getAppState()
  })

  onDestroy(() => {
    unlistenAuthComplete?.()
    unlistenAuthError?.()
  })

  function handleSetupComplete() {
    authError = null
    appState = 'Unauthenticated'
  }
</script>
```

Note: adjust event accessor names (`authCompleteEvent`, `authErrorEvent`) to match what `bindings.ts` actually exports — read the file first.

- [ ] **Step 3: Run svelte-check — green phase**

```bash
npm run check
```

Expected: zero errors in `App.svelte`.

- [ ] **Step 4: Commit**

```bash
cd ..
git add gui-frontend/src/App.svelte
git commit -m "feat(gui): migrate App.svelte to TypeScript with typed bindings"
```

---

## Task 6: Migrate `Syncing.svelte` to TypeScript

**Files:**
- Modify: `gui-frontend/src/lib/Syncing.svelte`

- [ ] **Step 1: Add `lang="ts"` — red phase**

Change `<script>` to `<script lang="ts">` and run:
```bash
cd gui-frontend && npm run check
```

Expected: type errors about untyped `status`, `lastSync`, and the listen/invoke calls.

- [ ] **Step 2: Replace imports and add types — green phase**

Replace the `<script lang="ts">` block:

```typescript
<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from '../bindings'
  import type { SyncState } from '../bindings'

  let status: SyncState = 'Idle'
  let lastSync: string | null = null

  let unlistenSyncStatus: (() => void) | undefined

  onMount(async () => {
    unlistenSyncStatus = await events.syncStatusEvent.listen((event) => {
      status = event.payload.status
      lastSync = event.payload.last_sync ?? null
    })
    commands.startSync()
  })

  onDestroy(() => {
    unlistenSyncStatus?.()
  })

  function formatLastSync(iso: string | null): string {
    if (!iso) return 'Never'
    try {
      return new Date(iso).toLocaleString()
    } catch {
      return iso
    }
  }
</script>
```

Note: adjust `syncStatusEvent` to match the actual exported name in `bindings.ts`. The `last_sync` field in the event payload is `Option<String>` on the Rust side — tauri-specta may generate it as `string | null` or `string | undefined`; adjust the `?? null` coercion accordingly.

- [ ] **Step 3: Run svelte-check — green phase**

```bash
npm run check
```

Expected: zero errors in `Syncing.svelte`.

- [ ] **Step 4: Commit**

```bash
cd ..
git add gui-frontend/src/lib/Syncing.svelte
git commit -m "feat(gui): migrate Syncing.svelte to TypeScript with typed bindings"
```

---

## Task 7: Migrate `Setup.svelte` to TypeScript

**Files:**
- Modify: `gui-frontend/src/lib/Setup.svelte`

- [ ] **Step 1: Add `lang="ts"` — red phase**

Change `<script>` to `<script lang="ts">` and run:
```bash
cd gui-frontend && npm run check
```

Expected: type errors about `instanceUrl`, `syncDir`, `error`, `submitting` variables.

- [ ] **Step 2: Replace imports and add types — green phase**

Replace the `<script lang="ts">` block:

```typescript
<script lang="ts">
  import { createEventDispatcher } from 'svelte'
  import { commands } from '../bindings'

  const dispatch = createEventDispatcher()

  let instanceUrl: string = ''
  let syncDir: string = ''
  let error: string | null = null
  let submitting: boolean = false

  async function handleSubmit() {
    submitting = true
    error = null
    try {
      await commands.initConfig(instanceUrl, syncDir)
      dispatch('complete')
    } catch (e) {
      error = String(e)
    } finally {
      submitting = false
    }
  }
</script>
```

- [ ] **Step 3: Run svelte-check — green phase**

```bash
npm run check
```

Expected: zero errors in `Setup.svelte`.

- [ ] **Step 4: Commit**

```bash
cd ..
git add gui-frontend/src/lib/Setup.svelte
git commit -m "feat(gui): migrate Setup.svelte to TypeScript with typed bindings"
```

---

## Task 8: Migrate `Auth.svelte` to TypeScript

**Files:**
- Modify: `gui-frontend/src/lib/Auth.svelte`

- [ ] **Step 1: Add `lang="ts"` — red phase**

Change `<script>` to `<script lang="ts">` and run:
```bash
cd gui-frontend && npm run check
```

Expected: type errors about the untyped `error` prop.

- [ ] **Step 2: Replace imports and add types — green phase**

Replace the `<script lang="ts">` block:

```typescript
<script lang="ts">
  import { onMount } from 'svelte'
  import { commands } from '../bindings'

  export let error: string | null = null

  onMount(() => {
    commands.startAuth()
  })

  function retry() {
    error = null
    commands.startAuth()
  }
</script>
```

- [ ] **Step 3: Run svelte-check — green phase**

```bash
npm run check
```

Expected: zero errors across all components.

- [ ] **Step 4: Final build verification**

```bash
npm run build
```

Expected: build succeeds with no errors.

- [ ] **Step 5: Commit**

```bash
cd ..
git add gui-frontend/src/lib/Auth.svelte
git commit -m "feat(gui): migrate Auth.svelte to TypeScript with typed bindings"
```

---

## Task 9: Final verification

- [ ] **Step 1: Run all Rust tests**

```bash
cargo test -q
```

Expected: all tests pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 3: Run svelte-check one final time**

```bash
cd gui-frontend && npm run check
```

Expected: zero errors across all four components.

- [ ] **Step 4: Run frontend build**

```bash
npm run build
```

Expected: production build succeeds.
