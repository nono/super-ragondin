# GUI End-to-End Tests Design

## Goal

End-to-end tests for the Svelte GUI running inside Tauri, using WebDriver
with screenshots, written in Rust with the `thirtyfour` crate.

## Approach

Use Tauri's official WebDriver support: `tauri-driver` wraps the platform's
native WebDriver server (`WebKitWebDriver` on Linux). The Rust `thirtyfour`
crate connects as a WebDriver client to interact with the running app.

## Architecture

```
┌──────────────┐   WebDriver    ┌───────────────┐   WebKitWebDriver   ┌──────────────────┐
│  thirtyfour  │ ──────────────►│ tauri-driver  │ ────────────────────►│ super-ragondin   │
│  (test)      │   port 4444    │               │                     │ (debug binary)   │
└──────────────┘                └───────────────┘                     └──────────────────┘
```

## Structure

```
crates/gui-e2e/
├── Cargo.toml
├── screenshots/          # saved PNGs (gitignored)
├── src/
│   └── lib.rs            # helpers: start tauri-driver, connect, screenshot
└── tests/
    └── setup_screen.rs   # first test: Setup screen renders correctly
```

## Prerequisites

- `cargo install tauri-driver --locked`
- `apt install webkit2gtk-driver` (provides `WebKitWebDriver`)
- App built: `cargo build -p super-ragondin-gui`

## Running

```bash
cargo test -p gui-e2e -- --ignored
```

Tests are `#[ignore]` so they don't run during regular `cargo test`.

## First test: Setup screen

1. Spawn `tauri-driver` on port 4444
2. Connect `thirtyfour` with `browserName: "wry"` and `tauri:options.application`
3. Wait for Setup screen to load
4. Assert: "Super Ragondin" heading is visible
5. Assert: 3 input fields present (URL, sync dir, API key)
6. Assert: "Connect to Cozy →" button present
7. Take screenshot → `screenshots/setup_screen.png`
8. Compare screenshot against committed baseline in `references/setup_screen.png` (1% pixel-diff tolerance)
   - If `UPDATE_SNAPSHOTS=1` is set, overwrite the baseline instead of comparing
8a. Quit driver, kill `tauri-driver`
