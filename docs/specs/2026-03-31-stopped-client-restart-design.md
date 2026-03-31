# Stopped-Client Restart Design

**Date:** 2026-03-31

## Problem

When Super Ragondin is stopped and the user makes local filesystem changes (add, delete, or replace files), those changes are not correctly detected on the next sync cycle after restart.

### Root cause

`SyncEngine::initial_scan` inserts newly scanned local nodes into the store but never removes local nodes that are no longer on disk. This leaves stale local nodes in the store after restart, causing the planner to mis-reason about three scenarios:

| Scenario | Symptom without fix |
|---|---|
| File added while stopped | Should work (scanner finds new inode, planner uploads) — but masked by the same stale-node problem in compound cases |
| File deleted while stopped | Stale local node stays in store; planner sees `(local=stale, remote=exists, synced=exists)` → NoOp; `DeleteRemote` never generated |
| File replaced while stopped (delete A, create B at same path) | Stale node for A stays; new node B inserted; planner sees a `NameCollision` conflict for B because A's synced record still points to the same remote path → B gets a conflict-rename instead of cleanly replacing A |

The simulator's `SimulationRunner::reconcile_local()` already implements the correct behaviour: it removes store nodes absent from the mock filesystem, then inserts all current nodes. The real `initial_scan` must be brought in line with this.

## Fix

### `SyncEngine::initial_scan` (`crates/sync/src/sync/engine.rs`)

After inserting scanned nodes, collect the set of scanned `LocalFileId`s (plus the root id), then delete any local node in the store whose id is not in that set.

Execution order:
1. `bootstrap_root` — establish root synced record (unchanged)
2. Scan filesystem — unchanged
3. **Safety check** (abort if scan is empty but store has synced records) — unchanged, runs before deletion
4. Insert all scanned nodes — unchanged
5. **NEW:** delete local store nodes whose id was not seen in the scan

This mirrors `reconcile_local()` in the simulator exactly.

## Tests

### `crates/sync/tests/sync_tests.rs` — unit tests against real `SyncEngine`

Three new tests. Each test:
- Creates a real temp directory
- Syncs an initial state
- Simulates a restart by dropping the engine and opening a fresh `SyncEngine` from the same store path
- Performs the offline change directly on the filesystem
- Calls `initial_scan` then `plan`
- Asserts the correct sync operations are planned

| Test name | Scenario | Expected plan |
|---|---|---|
| `initial_scan_picks_up_file_added_while_stopped` | New file created on disk while engine not running | `UploadNew` |
| `initial_scan_detects_file_deleted_while_stopped` | Previously-synced file removed from disk while engine not running | `DeleteRemote` |
| `initial_scan_detects_file_replaced_while_stopped` | Previously-synced file deleted and a new file created at the same path (new inode) while engine not running | `DeleteRemote` for old + `UploadNew` for new (no conflict) |

### `crates/sync/tests/simulator_tests.rs` — simulation tests

Three new targeted simulation tests using `StopClient` / `RestartClient` / `Sync` and asserting `check_convergence`:

| Test name | Scenario |
|---|---|
| `simulation_file_added_while_stopped_syncs_on_restart` | Stop → create local file → restart → sync → converged |
| `simulation_file_deleted_while_stopped_syncs_on_restart` | Sync file → stop → delete local file → restart → sync → converged |
| `simulation_file_replaced_while_stopped_syncs_on_restart` | Sync file → stop → delete + create new file at same name → restart → sync → converged |

The existing proptest (`prop_arbitrary_action_sequence_converges`) already calls `RestartClient` before the final convergence check. No proptest changes are needed.

## Out of scope

- Remote changes while stopped (already handled by `fetch_and_apply_remote_changes`)
- Directory-level variants of the three scenarios (directories follow the same code path and are covered by the proptest)
