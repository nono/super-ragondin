# GitHub Actions CI/CD

Continuous integration and release workflows.

## Workflows

- `.github/workflows/ci.yml` - CI pipeline (build, test, lint, E2E)
- `.github/workflows/release.yml` - Release builds
- `.github/actions/install-system-deps/` - Custom action for system dependencies

## Findings

- Avoid `restore-keys` when caching single binaries installed via `cargo install` — prefix matches restore the old binary but set `cache-hit=false`, causing `cargo install` to fail on the existing file
- Use `--locked` with `cargo install` to avoid version drift and ensure reproducible binary installations
- Use `Swatinem/rust-cache@v2` with `shared-key` + `save-if: false` on secondary jobs to share compiled deps without cache thrashing
- Tauri's `beforeBuildCommand` only runs via `cargo tauri build` — in CI with `cargo build`, the frontend must be built manually first (`npm run build` in `gui-frontend/`)
- Release builds must target both binaries: `cargo build --release --locked --bin super-ragondin --bin super-ragondin-gui`
- GPU-related warnings (`libEGL`, `MESA-LOADER`, `ZINK`) in headless CI environments are harmless and can be ignored
