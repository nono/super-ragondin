# Frontend, GUI & E2E Tests

Tauri v2 desktop app with a Svelte 5 frontend.

## Crate Structure

### `crates/gui/` (`super-ragondin-gui`)

Tauri v2 desktop GUI binary:

- `src/main.rs` - Tauri builder setup, registers commands and managed state
- `src/commands.rs` - All Tauri commands (`get_app_state`, `init_config`, `start_auth`, `start_sync`) + sync loop
- `tauri.conf.json` - Window size, frontend paths, app identifier
- `capabilities/default.json` - Tauri capability declarations

### `gui-frontend/` — Svelte 5 + Vite

- `src/App.svelte` - State machine (Unconfigured → Unauthenticated → Ready)
- `src/lib/Setup.svelte` - Setup form (instance URL + sync directory)
- `src/lib/Auth.svelte` - OAuth wait screen with retry
- `src/lib/Syncing.svelte` - Sync status screen

### `crates/gui-e2e/` (`gui-e2e`)

GUI end-to-end tests via WebDriver:

- `src/lib.rs` - Helpers: `start_tauri_driver()`, `connect_driver()`, `save_screenshot()`, `ConfigGuard` (writes/restores test config)
- `tests/setup_screen.rs` - Setup screen (Unconfigured state) rendering test with screenshot
- `tests/auth_screen.rs` - Auth screen (Unauthenticated state) rendering test; uses a TCP listener to keep OAuth in waiting state
- `tests/main_layout_screen.rs` - MainLayout screen (Ready state) rendering test; uses a fake config with access token

## Commands

```bash
cargo build -p super-ragondin-gui --no-default-features --features custom-protocol  # Build GUI binary for E2E tests
xvfb-run cargo test -p gui-e2e -- --ignored                                         # Run GUI E2E tests (requires tauri-driver + WebKitWebDriver + xvfb)
UPDATE_SNAPSHOTS=1 xvfb-run cargo test -p gui-e2e -- --ignored                      # Update visual regression baselines
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `UPDATE_SNAPSHOTS` | unset | Set to `1` to overwrite visual regression baselines instead of comparing |

## Findings

- Tauri v2 on Linux requires system packages: `pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev libssl-dev` — install via `sudo apt install` before building `crates/gui`
- Tauri v2 E2E builds require `--features custom-protocol` so the binary embeds the frontend (without it, `cfg(dev)=true` and the binary tries to connect to `devUrl` at runtime)
- Tauri v2 custom commands registered via `invoke_handler` are covered by `core:default` in capabilities — no per-command capability entries needed
- Svelte 5 components with runes must be mounted via `mount()` (not legacy `new App()`); the legacy API throws `effect_orphan` in WebKitWebDriver automation mode
- After `connect_driver()`, call `driver.goto("tauri://localhost")` to ensure a clean page load with Tauri's JS bridge properly injected
- Tauri v2 E2E tests on Linux use `tauri-driver` + `WebKitWebDriver` (package `webkit2gtk-driver`) with `thirtyfour` as the Rust WebDriver client
- Tauri's `beforeBuildCommand` only runs via `cargo tauri build` — in CI with `cargo build`, the frontend must be built manually first (`pnpm run build` in `gui-frontend/`)
