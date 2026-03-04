# Cozy Desktop Comparison Report

A deep analysis of the old client (`cozy-desktop`, Node.js/Electron) and the new
client (`cozy-desktop-ng`, Rust) to identify missing features, portable test
scenarios, tricks worth learning, and tooling gaps.

---

## 1. Features Missing in `cozy-desktop-ng`

### 1.1 GUI, System Tray, and Desktop Integration

The old client ships a full Electron GUI with:
- System tray icon with status indicator (buffering, syncing, up-to-date, offline,
  error, user-alert).
- Onboarding window (OAuth registration flow with browser interaction).
- Tray window showing recent files, sync progress, and user alerts.
- Help/feedback window.
- Auto-updater window (via `electron-updater`).
- Desktop notifications for errors and sync events.
- Single-instance lock (`app.requestSingleInstanceLock()`).
- Power management: suspends sync on sleep, resumes on wake.
-
The new client has only a CLI with `init`, `auth`, `sync`, `watch`, and `status`
commands.

### 1.2 Cross-Platform Support (Windows and macOS)

The old client runs on Windows, macOS, and Linux with platform-specific code:

**Windows:**
- `@gyselroth/windows-fsstat` for stable file IDs (Node.js `fs.stat()` inodes
  are unreliable on NTFS).
- `winIdenticalRenaming` watcher step for case-only renames.
- `attrib +h` for hidden directories (temp dir, system dirs).
- NTFS path restrictions: reserved chars (`<>:"/\|?*`), reserved names (CON,
  PRN, AUX, NUL, COM1-9, LPT1-9), forbidden trailing `.` and space, 256-byte
  name limit (243 for dirs), 32766-byte path limit.
- Executable bit always `false`.
- Windows date migration flag for timestamp precision changes.
- NSIS installer with DigiCert KeyLocker code signing.

**macOS:**
- Chokidar watcher (APFS/HFS+ aware).
- NFD Unicode normalization in path IDs (`path.normalize('NFD').toUpperCase()`).
- Case-insensitive ID computation for IdConflict detection.
- `normalizePaths` step in remote watcher.
- DMG + ZIP packaging with hardened runtime and notarization.

The new client only supports Linux (inotify watcher, case-sensitive paths, no
platform incompatibility detection).

### 1.3 Cozy Notes Integration

The old client treats `.cozy-note` files specially:
- MIME type detection: `text/vnd.cozy.note+markdown`.
- `isNote()` checks for mime type plus metadata (content, schema, title,
  version).
- Opening notes: `findNote()` looks up the PouchDB record, fetches the remote
  URL via `cozy-client`'s `models.note.fetchURL()`, and opens in the browser.
- If the note is not found remotely, parses the local tar archive for markdown
  content.
- EPERM workaround: old Cozy Notes with read-only permissions need
  `fse.move(overwrite: true)` instead of `fs.rename()`.
- File association: `.cozy-note` files registered with custom MIME type.
- Note metadata stripping: `removeNoteMetadata()` strips `content`, `schema`,
  `title`, `version` from PouchDB documents to reduce size.

The new client treats notes as generic files.

### 1.4 OS-Native Trash

The old client uses the OS trash:
- `shell.trashItem()` (Electron API) sends to the platform's native trash
  (Recycle Bin, macOS Trash, freedesktop Trash).
- Falls back to `fse.remove()` (permanent deletion) if trash fails.
- Handles `ENOENT` (file already gone) gracefully.
- Custom `MissingFileError` class to distinguish "already deleted" from "trash
  operation failed".

The new client uses `fs::remove_file()` / `fs::remove_dir_all()` for local
deletions -- files are permanently destroyed, not recoverable from the system
trash.

### 1.5 Conflict File Naming

The old client generates readable conflict names:
- Pattern: `filename-conflict-2024-01-15T12_30_45.123Z.ext`
- Colons replaced with underscores for Windows compatibility.
- Filename truncated to 180 characters before appending suffix (prevents
  exceeding filesystem name limits).
- If a file already has a conflict suffix, it is *replaced* rather than
  accumulated (prevents `-conflict-...-conflict-...`).
- Extension is preserved after the suffix.

The new client reports conflicts as `Conflict` variants in `PlanResult` but does
not generate renamed conflict copies. The 8 conflict kinds (`BothModified`,
`LocalDeleteRemoteModify`, `LocalModifyRemoteDelete`, `ParentMissing`,
`NameCollision`, `BothMoved`, `InvalidName`, `CycleDetected`) are logged but the
user must resolve them manually.

### 1.6 Selective Sync (Differential Sync)

The old client supports excluding directories from sync:
- Remote directories have a `not_synchronized_on` attribute containing a list of
  OAuth client IDs.
- `isExcludedDirectory()` checks whether the current client is in the exclusion
  list.
- `includeInSync()` removes the client ID to re-include a directory.
- When re-included, `needsContentFetching = true` triggers recursive content
  fetching.
- User action `link-directories` in the GUI re-includes excluded folders.

The new client syncs everything under the root directory with no exclusion
mechanism.

### 1.7 Ignore Rules (syncignore)

The old client has a gitignore-compatible ignore system:
- Default rules in `core/config/syncignore` ignore: hidden files (`.*`), Dropbox
  files, editor temp files (Emacs `*~`, Vim `*.sw[px]`, LibreOffice
  `.~lock.*#`, MS Office `~$*`), Chrome downloads (`.crdownload`), OS files
  (`.DS_Store`, `Thumbs.db`, `$Recycle.bin`), etc.
- User-customizable rules in `~/.twake-desktop/syncignore`.
- Case-insensitive matching on macOS and Windows.
- Three-checkpoint strategy: checked when local change detected, when applying
  to local FS, and when applying to remote.
- Supports negation (`!pattern`), basename matching, folder-only matching
  (trailing `/`), and `**` globstar.
- Recursive parent matching for patterns like `node_modules/`.

The new client has no ignore mechanism. All files (except symlinks and special
files) are synced.

### 1.8 Platform Incompatibility Detection

The old client detects and blocks files that would be invalid on the local
platform:
- Windows: reserved characters, forbidden trailing chars, reserved names
  (CON, PRN, etc.), path/name byte-length limits.
- macOS: path max 1023 bytes, name max 255 bytes.
- Linux: path max 4095 bytes, name max 255 bytes.
- Name length checked in *bytes* (via `Buffer.byteLength()`), not characters.
- Recursive ancestor checking: `/path/CON/file.txt` is caught even though
  `file.txt` itself is valid.
- Incompatible documents are marked with `doc.incompatibilities` and throw
  `IncompatibleDocError` during sync.

The new client only validates against path traversal characters (`.`, `..`, `/`,
`\`, `\0`) via the `InvalidName` conflict. It does not check platform-specific
restrictions, byte-length limits, or reserved names.

### 1.9 Quota / Disk Space Awareness

The old client checks disk usage:
- `diskUsage()` API call to check remote quota/used space.
- `ENOSPC` local errors mapped to `NO_DISK_SPACE_CODE` with 1-minute retry.
- `FileTooLarge` (>5GB Swift limit) detected client-side before upload.
- GUI displays storage usage and alerts.

The new client has no quota checking, disk space awareness, or file size limits.

### 1.10 Shared Drives / Permissions

The old client supports opening files in the Cozy Drive web interface:
- `findDocument()` generates web links using `cozy-client`'s
  `generateWebLink()`.
- Supports flat vs. nested subdomain configurations.
- `capabilities()` API call to check subdomain style.

The new client has no concept of shared drives or web links.

### 1.11 Executable Permission Sync

The old client syncs executable permissions:
- Files track an `executable` boolean.
- On Windows, `executable` always defaults to `false`.
- Changes are detected and synced in both directions.
- Moves preserve the executable flag.

The new client does not track or sync file permissions.

### 1.12 Realtime Notifications

The old client uses WebSocket-based realtime notifications:
- `cozy-realtime` library for instant change detection.
- Complements the polling-based changes feed.
- Queue-based processing to prevent concurrent operations.

The new client relies solely on polling (30-second periodic sync in watch mode).

### 1.13 Error Reporting (Sentry)

The old client integrates Sentry (`@sentry/electron`) for production error
reporting. The new client logs to files but has no external error reporting.

### 1.14 Auto-Update

The old client uses `electron-updater` for automatic updates with platform-
specific installers (AppImage, DMG, NSIS). The new client has no update
mechanism.

### 1.15 Proxy Support

The old client supports HTTP, HTTPS, SOCKS, and PAC proxies via
`pac-proxy-agent`. The new client has no proxy support.

### 1.16 OAuth Token Refresh Callback

The old client has an `onTokenRefresh()` callback that auto-persists new tokens
when they are refreshed during API calls. The new client handles token refresh
in `CozyClient` but the integration with automatic config persistence during
long-running sync is less robust.

### 1.17 Warnings and User Action Required

The old client handles HTTP 402 responses for:
- Terms of Service updates requiring acceptance.
- Payment issues requiring user action.
- These are surfaced in the GUI as `user-alert` states.
The new client does not detect or handle 402 responses.

### 1.18 Log Upload and Support Tools

The old client can:
- Upload gzipped logs and PouchDB tree to `desktop-upload.cozycloud.cc`.
- Send support emails via Cozy's sendmail job with debug info.
- Include client version, config, OS info, permissions in support data.

The new client has structured JSONL logging with daily rotation but no support
upload mechanism.

---

## 2. Test Scenarios Worth Porting

### 2.1 High Priority: Move Scenarios

The old client has ~40 move-related scenarios. These are the most complex and
bug-prone operations. Priority candidates:

**Successive / chained moves:**

- `move_file_successive` -- Move src->dst1, wait, dst1->dst2. Tests that the
  planner doesn't lose track of the file after the first move.
- `move_dir_successive_same_level` -- Two rapid successive dir moves. Tests
  debouncing and event ordering.
- `move_file_a_to_b_to_c_to_b` -- Cyclic rename (back to an intermediate
  name). Tests that the planner handles name reuse correctly.

**Move + create (replacing the source):**

- `move_a_to_b_and_create_a` -- Move file then recreate at original path.
  Critical for atomic-save detection. Tests that the new file at the old path
  is not confused with the moved file.
- `move_dir_a_to_b_and_create_a` -- Same but for directories.

**Nested moves:**

- `move_file_inside_move` -- Rename a file, then move its parent directory,
  then rename another file inside. Tests cascading path updates.
- `move_from_inside_move` -- Move a child out of a moved parent. Tests that
  both moves are tracked independently.
- `move_dir_parent_and_child` -- Move parent, rename child, rename grandchild.
  Cascading move dependencies.

**Move + trash:**

- `move_and_trash_file` -- Move then immediately trash. Tests that the move
  isn't applied after the file is gone.
- `trash_file_and_move_parent` -- Trash a child then move the parent. Tests
  dependency ordering (child trash before parent move).
- `move_dir_content_and_trash_dir` -- Extract content from a dir, then trash
  the container. Tests that content is preserved.

**Move + update:**

- `move_and_update_file` -- Move + content update in quick succession. The new
  client's planner supports co-existing move and content operations, but it
  hasn't been tested with rapid filesystem events.
- `move_dir_and_replace_subfile` -- Dir move, then replace a child file via
  the temp-file pattern. Tests atomic save inside a moved directory.

**Overwriting moves:**

- `move_overwriting_file/trashed_first` -- Force-move to a path where another
  file exists. Tests that the overwritten file ends up in trash.
- `move_overwriting_dir/trashed_first` -- Complex dir overwrite with
  sub-files. Tests recursive merge/replace behavior.

**File swap pattern:**

- `move_files_a_to_c_and_b_to_a` -- Move A away, then move B to A's old path.
  Tests that paths are tracked by identity (inode), not by name.

### 2.2 High Priority: Conflict Scenarios

From integration tests:
- **Concurrent edits** (`conflict_resolution.js`): Both sides modify the same
  file. Tests: local-merged-first vs. remote-merged-first, with version
  checking (was the local content already versioned on remote?).
- **File vs. directory name collision**: A file and directory created at the
  same path on different sides.
- **Case/encoding conflicts** (`id_conflict.js`): `file.txt` vs. `FILE.TXT`,
  NFD vs. NFC normalization. Platform-specific behavior.
- **Move destination collision**: File addition vs. file move to the same
  path.

### 2.3 High Priority: Application-Specific Patterns

- `ods_update_through_osl_tmp_file` -- LibreOffice's save pattern:
  rename `file.ods` -> `file.ods.osl-tmp`, create new `file.ods`, update it,
  delete the tmp. This is the classic atomic save pattern that users
  encounter daily.
- `update_through_unignored_tmp_file` -- Generic application save-through-
  temp-file pattern.

The new client has atomic save detection in the planner (inode reconciliation)
but it needs to be validated against these real-world application behaviors.

### 2.4 Medium Priority: Trash and Delete Scenarios

- `add_trash_file` / `add_trash_dir` -- Create then immediately trash. Tests
  that transient documents are not synced.
- `delete_dir_permanently` -- Permanent deletion of a directory hierarchy.
  Tests cascade delete behavior.
- `move_and_trash_file_from_inside_move` -- Trash a child after parent move.
- `replace_file_with_file` -- Delete a file and create a new one at the same
  path. Tests that the planner treats this as a replacement, not an update.

### 2.5 Medium Priority: Case and Encoding Scenarios

- `rename_identical` -- Case-only rename (e.g., `Foo` -> `FOO`) and
  NFC<->NFD encoding change. This is a no-op on case-insensitive FS but
  meaningful on Linux.
- `create_dir_with_utf8_character` -- UTF-8 directory names with NFC/NFD
  handling.
- `create_accentuated_file_in_accentued_dir/with_different_encodings` --
  Mixed NFC/NFD encoding in the same path.
- `change_unmerged_doc_case` -- Case changes on documents not yet synced.

### 2.6 Medium Priority: Stopped Client Scenarios

Every scenario in the old client is run in a `STOPPED_CLIENT` mode where
actions happen while the client is stopped, then changes are detected on
restart via initial scan. This is a critical testing dimension that the new
client's `initial_scan()` should be validated against:
- Files created, moved, or deleted while offline.
- Directory renames while offline (detected via inode matching).
- Multiple moves while offline (only the final state is visible on restart).

### 2.7 Medium Priority: Integration Test Scenarios

From `test/integration/`:
- **`interrupted_sync.js`** -- Sync interrupted mid-operation. Tests that
  restart recovers gracefully.
- **`update.js`** -- Race conditions: offline changes with unsynced previous
  changes, M1-merge-M2 race, retry behavior after 3 failures then new update.
- **`full_loop.js`** -- Full watch/merge/sync loop with inode and side
  tracking verification.
- **`mtime-update.js`** -- Modification-time-only changes (should not trigger
  re-upload if content is unchanged).

### 2.8 Lower Priority: Property-Based Testing Enhancements

The old client has two property-based test suites:

**Local watcher fuzzing** (`test/property/local_watcher/`):

- Generates random sequences of filesystem operations (mkdir, create, update,
  mv, rm, reference, start/stop/restart watcher, sleep).
- Verifies PouchDB tree matches actual filesystem after all operations.
- 18 recorded scenario files with names like `cycle_rename.json`,
  `advanced_moves.json`, `deletions.json`.

**Two-client sync** (`test/property/two_clients/`):

- Starts cozy-stack, registers two OAuth clients.
- Runs operations on both clients in parallel.
- Waits for convergence, verifies both devices match the Cozy.

The new client already has excellent property-based testing via proptest, but
it could benefit from:
- Adding filesystem operation sequences from the old client's recorded
  scenarios as regression seeds.
- Testing two-client convergence (the simulator currently uses one client).
- Adding more action types: case renames, encoding changes, ignored files.

### 2.9 Lower Priority: Regression-Specific Tests

The old client has two regression tests for specific bugs:
- **TRELLO_484**: Move file into dir, rename dir, then rename moved file.
  Tests that folder move is squashed correctly and applied before child
  moves.
- **TRELLO_646**: Move `src/` -> `dst/` locally, then remote polling occurs
  before the move is synced. Tests that polling does not recreate source
  metadata (which would break the pending move).

Both test timing-sensitive interactions between watchers and sync that could
occur in the new client.

---

## 3. Tricks the New Client Should Learn

### 3.1 Staging Area for Downloads

**Old client** (`core/local/index.js`):
Downloads go to `.system-tmp-twake-desktop/` inside the sync folder, with a
`.tmp` extension. After download:
1. Compute checksum of temp file and verify against `doc.md5sum`.
2. Verify file size matches `doc.size`.
3. Only then: `fs.rename()` to the final path.
4. On any failure: clean up the temp file.

**New client** (`src/sync/engine.rs` lines 875-884):
Already uses a staging pattern (write to `staging_dir/{uuid}`, then
`fs::rename()`), and verifies MD5 after download. But it is missing:
- **Size verification** after download (guards against cozy-stack corruption
  where the checksum might match but the size differs).
- **Temp file cleanup on error** (the staging file may be orphaned if an
  error occurs between download and rename).

### 3.2 Await Write Finish (Don't Sync Files Still Being Written)

**Old client** (`core/local/channel_watcher/await_write_finish.js`):
A critical pipeline stage that debounces file write events with a 200ms delay.
Key behaviors:
- Holds file events in a queue, only releasing them when no more write
  candidates exist.
- Aggregates rapid successive events: `create + modify = create`,
  `rename + delete = delete at oldPath`, `rename + modify = rename`.
- Uses inode matching to correctly aggregate events across renames.
- Cross-batch debouncing: if a write spans two event batches, the earlier
  event is spliced out.
- Created-then-deleted within a batch = temporary file, both events dropped.

**New client** (`src/main.rs` lines 160-210):
Uses a 2-second debounce on inotify events before running a sync cycle. This
is a simpler approach, but it has weaknesses:
- A 2-second debounce may not be enough for large file writes.
- No event-level aggregation: the scanner re-reads the entire directory tree
  on every cycle, which may observe files mid-write.
- The TOCTOU protection in `scanner.rs` (re-stat after hashing, skip if
  size/mtime/inode changed) partially mitigates this, but a file that is
  being written in a pattern that doesn't change size between reads could
  still be synced prematurely.

**Recommendation**: Consider implementing a per-file debounce mechanism that
waits for `CLOSE_WRITE` inotify events before considering a file ready for
sync. The inotify watcher already captures `CLOSE_WRITE` events (line 63 of
`src/local/watcher.rs`) but the sync engine doesn't use this signal.

### 3.3 Timestamp Precision Handling

**Old client** (`core/utils/timestamp.js`):
- `fromDate()` truncates milliseconds to 0.
- `almostSameDate()` uses a 3-second tolerance window to handle:
  - FAT32's 2-second timestamp resolution.
  - Network delays during metadata updates.
  - Clock drift between local and remote.
- `roundedRemoteDate()`: When cozy-stack returns nanosecond-precision dates
  (up to 8 fractional digits), and JS truncates to milliseconds, the result
  is always *lower* than the original. Cozy-stack rejects updates with
  "older" timestamps, so the old client adds 1ms when truncation occurs.
- `stringify()` truncates ISO strings to second precision for deterministic
  comparison.
- `assignMaxDate()` ensures `updated_at` never goes backward.

**New client** (`src/model.rs`):
- `updated_at` is stored as `i64` (Unix epoch seconds) -- already truncated
  to seconds.
- No tolerance window for comparison.
- No rounding-up logic for remote dates.

**Recommendation**: The new client should:
1. Be aware that `updated_at` values from the Cozy API may have nanosecond
   precision and need rounding up (not down) when comparing.
2. Use a tolerance window (at least 1-3 seconds) when comparing timestamps
   to avoid false-positive change detection.
3. Ensure `updated_at` never goes backward when merging metadata.

### 3.4 Checksum Computation: Serial Queue and EBUSY Retry

**Old client** (`core/local/checksumer.js`):
- Checksums are computed one at a time using `async.queue()` with concurrency
  1. This is better for HDD performance because sequential reads are faster
  than random seeks from concurrent hashing.
- EBUSY errors (Windows file locking) are retried with exponential backoff:
  1s, 2s, 4s, 8s, 16s (5 retries).

**New client** (`src/local/scanner.rs`):
- Checksums are computed sequentially during the scan (one file at a time,
  inherently serial). Good.
- No retry for locked/busy files. On Linux, EBUSY is rare, but if Windows
  support is added, this will be needed.

### 3.5 Local File Reuse by Checksum (Avoid Re-Download)

**Old client** (`core/local/index.js`):
Before downloading a file from the remote, checks if any local file with the
same MD5 checksum already exists. If so, copies locally instead of
downloading, saving bandwidth.

**New client**: Always downloads from the remote. No checksum-based
deduplication.

**Recommendation**: This is a significant bandwidth optimization, especially
for files that were moved on the remote (different path, same content). The
new client could check the local tree for a node with the matching `md5sum`
before issuing a download.

### 3.6 Protect Against Empty Sync Directory

**Old client** (`core/local/channel_watcher/initial_diff.js`):
If the initial scan finds zero files but PouchDB has many records, the client
assumes the sync directory is on a disconnected mount point (e.g., external
drive) and calls `fatal()` to prevent deleting everything on the remote.

**New client**: No such safety check. If the sync directory is empty (e.g.,
the drive is unmounted), the scanner would report no local files, and the
planner would generate `DeleteRemote` operations for everything.

**Recommendation**: Before applying any sync cycle that would delete more than
a threshold percentage of known files, verify that the sync directory is
accessible and non-empty (or at least confirm with the user).

### 3.7 Inode Reuse Detection on Linux

**Old client** (`core/local/channel_watcher/initial_diff.js`):
Linux can reuse inodes after deletion. If an inode lookup finds a PouchDB
record but the `kind` (file vs. directory) differs, the old client treats it
as inode recycling (not a rename): emits a `deleted` event for the old
document and treats the new one as a creation.

**New client** (`src/planner.rs` lines 128-191):
The atomic save detection matches by `(parent, name, type)` which implicitly
handles type mismatches. But the `LocalFileId` is `(device_id, inode)`, and
if an inode is reused by a different file at a different path with the same
type, the planner might incorrectly identify it as the same file.

**Recommendation**: When matching by `LocalFileId`, verify that the node type
matches before treating it as an identity match. Consider adding a secondary
check (e.g., content hash) for files that match by inode but have unexpected
metadata differences.

### 3.8 Incomplete Event Handling

**Old client** (`core/local/channel_watcher/incomplete_fixer.js`):
When a file is created and immediately moved, the watcher sees a `created`
event at the old path but the file no longer exists there. The event is marked
`incomplete`. The fixer holds it for 3 seconds, waiting for a subsequent
`renamed` event to complete it. If the rename arrives, the event is rebuilt
with the new path.

**New client**: The scanner-based approach avoids this problem because it reads
the current state of the filesystem rather than relying on event streams.
However, the inotify watcher (`src/local/watcher.rs`) does report individual
events. If the new client ever moves to an event-driven model (rather than
scan-on-change), this pattern will be needed.

### 3.9 Overwrite Detection (Delete + Rename = Overwriting Move)

**Old client** (`core/local/channel_watcher/overwrite.js`):
Holds batches for 500ms to detect the pattern: `delete` event for path X
followed by `renamed` event targeting path X. The `deleted` event is
suppressed and the `renamed` event gains an `overwrite: true` flag.

**New client**: The scanner-based approach sees only the final state (the new
file at path X), so it doesn't need to correlate delete+rename events.
However, the synced tree still has the old file's identity at path X, and the
planner must correctly handle the replacement. The current planner does handle
`NameCollision` conflicts for this case, but overwriting moves (where the
intent is to replace, not conflict) may be misclassified.

### 3.10 Dependency-Ordered Change Application

**Old client** (`core/sync/dependency_graph.js`):
Changes from PouchDB are sorted via a directed acyclic graph:
- Parent creation before child creation.
- Child move before parent deletion.
- Move into new directory after the directory is created.
- Cycle detection prevents infinite loops (edges that would create cycles are
  silently dropped).

**New client** (`src/planner.rs` lines 645-659):
Operations are sorted by priority: creates first, then moves, then transfers,
then deletes, then conflicts. This is a simpler ordering that handles the
common cases but may not handle all dependency patterns (e.g., a move into a
newly created directory where the creation was planned in the same cycle).

**Recommendation**: The priority-based ordering works well with the scanner-
based approach because all operations are planned from the complete current
state. But edge cases may arise when the remote changes feed brings
interleaved creates and moves. Consider adding parent-before-child ordering
within each priority level.

### 3.11 Two-Phase Execution (Non-Delete Before Delete)

**Old client** (`core/sync/index.js`):
The `trashWithParentOrByItself()` method checks if a parent is also being
deleted. If so, child deletions are skipped because they'll be handled
recursively with the parent. This prevents redundant API calls and preserves
the tree structure in remote trash.

**New client** (`src/simulator/runner.rs` lines 830-853):
The simulator already implements two-phase execution (non-delete ops first,
then re-plan and execute deletes). This is good but only exists in the
simulator. The actual sync engine (`src/sync/engine.rs`) executes operations
in a single pass.

**Recommendation**: Port the two-phase execution from the simulator to the
production sync engine to prevent cascade deletions from orphaning files that
were moved out of a directory being deleted.

### 3.12 HTTP/2 Error Reconstruction

**Old client** (`core/remote/cozy.js`):
When a chunked-encoding HTTP/2 request fails (e.g., 413 file too large),
Chromium replaces the response with `net::ERR_HTTP2_PROTOCOL_ERROR`, losing
all error information. The old client works around this by running diagnostic
checks after the failure:
1. Is the filename already taken? (409)
2. Is the file too large or disk full? (413)
3. Did the byte count differ from Content-Length? (412)

**New client**: Uses `reqwest` with `rustls-tls` which should provide better
HTTP/2 error handling than Chromium's network stack. But it's worth being
aware of this pattern in case similar issues arise with other HTTP clients or
proxies.

### 3.13 Versioning-Aware Conflict Resolution

**Old client** (`core/merge.js`):
Before creating a conflict copy, the old client checks if the "losing" side's
content was already stored as an old version on the remote (via
`fileContentWasVersioned()`). If the content is already versioned, the
conflict is resolved silently by overwriting with the remote version -- no
conflict copy is created.

**New client**: Reports all `BothModified` as conflicts. No version checking.

**Recommendation**: This is a significant UX improvement. Many "conflicts"
occur when a user edits a file locally, the auto-save uploads it, and then
the user edits again before the sync completes. In these cases, the
intermediate version is already on the remote as an old version, and creating
a conflict copy is unnecessary clutter.

### 3.14 Move Semantics: `moveFrom` Attribute

**Old client** (`core/move.js`):
Moves are represented by setting `dst.moveFrom = src` on the destination
document. This carries the full source metadata, enabling:
- Detection of moves where the source was never synced (converted to
  creation at destination).
- Detection of moves where the source was trashed (converted to deletion).
- Child move tracking (`src.childMove = true` when moved with parent).
- Overwrite detection (`dst.overwrite = existingDocAtDst`).

**New client** (`src/model.rs` lines 280-314):
Moves are represented as explicit `MoveLocal` and `MoveRemote` operations
with `old_path` and `new_path` fields. This is simpler but loses the rich
metadata about the source document's state.

### 3.15 PouchDB Lock / Concurrent Access Prevention

**Old client** (`core/pouch/`):
A promise-based exclusive lock prevents concurrent Merge (from watchers) and
Sync (from applying changes) from modifying PouchDB simultaneously. This
prevents race conditions where a watcher event is merged while Sync is
applying the same change.

**New client**: The scanner-based approach with synchronous execution avoids
most concurrency issues, but the async sync engine (`run_cycle_async`) could
theoretically race with a concurrent watcher event triggering a new scan.

### 3.16 Remote Watcher: Skip Files Without Checksum

**Old client** (`core/remote/watcher/index.js`):
Files in the remote changes feed that have no `md5sum` are classified as
`IgnoredChange`. These are typically files still being uploaded by another
client or temporary server-side states.

**New client**: Does not explicitly filter out remote files without checksums.
If a remote file has no `md5sum`, the download would fail or produce an
empty file.

**Recommendation**: Skip remote file nodes that have no `md5sum` -- they
represent incomplete uploads.

### 3.17 Tags Preservation

**Old client** (`core/merge.js`):
When merging, if the new document has empty tags, the existing tags are
preserved. This prevents local updates from clearing remote tags.

**New client**: Does not track or preserve tags.

### 3.18 Recursive Folder Move with Child Path Updates

**Old client** (`core/merge.js`):
`moveFolderRecursivelyAsync()` updates all children's paths, handles
single-side children, updates local/remote attributes, and skips trashed
children (whose deletion at the original path is still valid).

**New client**: The planner generates individual `MoveLocal`/`MoveRemote`
operations for each node. Child path updates after a parent move happen
implicitly through the next scan cycle, which may miss children that were
moved while the parent was being moved.

---

## 4. Everything Else

### 4.1 CI / Continuous Integration

**Old client CI:**
- GitHub Actions: Linux (Ubuntu 22.04) + macOS (13/14) with matrix jobs for
  unit, integration, scenarios, and builds.
- AppVeyor: Windows CI with NSIS builds and DigiCert code signing.
- Travis CI: Legacy (still in repo but likely deprecated).
- Composite actions: `setup-cozy-stack`, `setup-couchdb`, `setup-dnsmasq`,
  `build-and-publish`.
- macOS CI uses Docker via `douglascamata/setup-docker-macos-action` and
  creates HFS+/APFS disk images for filesystem-specific testing.
- Scenario tests run in matrix: `stopped_client x fs_type`.
- Build artifacts: AppImage, DMG+ZIP (x64+ARM64), NSIS exe.

**New client CI:**
- Single GitHub Actions workflow (`.github/workflows/ci.yml`).
- Runs `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`.
- No Windows or macOS CI.
- No integration test CI (requires cozy-stack).
- No build/packaging CI.

**Gaps to address:**
- Add CI jobs for integration tests (with cozy-stack in Docker).
- Add Windows and macOS CI when those platforms are supported.
- Add a composite action for setting up cozy-stack (reuse the old client's
  `setup-cozy-stack` action pattern).
- Consider running property-based tests in CI (the old client runs them
  separately).

### 4.2 Documentation

**Old client documentation:**

Developer docs (`doc/developer/`):
- `setup.md` -- Full dev environment setup guide.
- `requirements.md` -- OS-specific requirements (Windows/Chocolatey,
  macOS/Homebrew, Fedora, Ubuntu), Docker setup.
- `test.md` -- Comprehensive testing guide: unit, integration, scenarios
  (capture-replay system), property-based testing, coverage, debug options.
- `debug.md` -- Debug logging, PouchDB/request debug, USR1 signal to list
  watched paths.
- `log_analysis.md` -- Guide to using jq filters for log analysis.
- `api_doc.md` -- JSDoc API documentation generation.
- Diagrams: sync workflow (`workflow.ditaa`), Linux watcher state machine
  (`linux_watcher.gv`), Windows watcher state machine (`win_watcher.gv`).

User docs (`doc/usage/`):
- Platform-specific install/run guides (Linux, macOS, Windows).
- Filesystem support matrix.
- `syncignore` usage.
- `inotify` limit configuration.
- Known limitations.

Other:
- `CODE_ORGANIZATION.md` -- Code structure overview.
- `KNOWN_ISSUES.md` -- Known bugs and platform-specific problems.
- `CHANGELOG.md` -- Release history.

**New client documentation:**
- `README.md` -- Project overview, build instructions, roadmap.
- `AGENTS.md` -- Instructions for AI coding agents.
- `docs/plans/` -- Architecture planning notes.
- `docs/clients-comparison.md` -- This report (previous version).

**Gaps to address:**
- Add developer setup guide (Rust toolchain, cozy-stack setup).
- Add testing guide (how to run unit/integration/property tests).
- Add architecture documentation (3-tree model, planner algorithm, sync
  engine flow).
- Add sync state diagrams.
- Document the inotify watcher pipeline and event handling.
- Document the simulator and property-based testing strategy.
- Document known limitations and platform-specific issues.

### 4.3 Developer Tools

**Old client dev tools:**
- Interactive REPL (`dev/repl.js`) with pre-loaded app objects.
- Capture tools (`dev/capture/`) for recording and replaying filesystem and
  remote events as JSON fixtures.
- 287-line jq filter library (`.jq`) for analyzing JSONL logs: filter by
  level, component, path, time range; detect issues, conflicts; format output.
- `dev/log2gource.js` for visualizing sync activity as a Gource animation.
- Automated OAuth registration (`dev/remote/automated_registration.js`) for
  CI: parses login pages, handles CSRF, does PBKDF2 passphrase hashing.
- Directory exclusion manager (`dev/remote/change-dir-exclusions.js`).
- Dev watch mode (`yarn watch`) with live rebuilding of core, CSS, Elm, JS.

**New client dev tools:**
- CLI commands (`init`, `auth`, `sync`, `watch`, `status`).
- The `status` command shows config, tree sizes, and pending operations.
- Structured JSONL logging with daily rotation.

**Gaps to address:**
- Add a jq filter library for analyzing the JSONL log files (the old
  client's `.jq` file is an excellent reference).
- Consider adding a REPL or interactive debug mode for inspecting the fjall
  store.
- Add automated OAuth registration for CI (to enable integration tests
  without manual browser interaction).
- Consider adding event capture/replay tooling for the inotify watcher.

### 4.4 Build and Packaging

**Old client:**
- electron-builder with platform-specific builds:
  - Linux: AppImage with custom launcher script (handles Chromium sandbox,
    registers `.cozy-note` MIME type).
  - macOS: DMG + ZIP for x64 and ARM64, code signing and notarization.
  - Windows: NSIS installer, DigiCert KeyLocker code signing.
- CSS: Stylus compiler with cozy-ui plugin.
- Elm: compiled to `gui/elm.js`.
- Translations: Transifex pull for 12 languages.

**New client:**
- `cargo build` produces a single Linux binary.
- No packaging, no installer, no code signing, no auto-update.
These will be needed when the client matures but are not immediate priorities.

### 4.5 Linting and Code Quality

**Old client:**
- ESLint with `cozy-app/basics` config, `prettier` integration.
- Flow (v0.108) for static type checking.
- elm-format for Elm code.
- EditorConfig for consistent formatting.
- `.gitattributes` forces LF for `.js` and `.sh` files.

**New client:**
- `cargo fmt` for formatting.
- `cargo clippy` with `all`, `pedantic`, and `nursery` lint groups.
- `unsafe_code = "forbid"`.
- No EditorConfig or `.gitattributes` (not needed for Rust single-language).
The new client's linting setup is already stronger than the old client's.
Clippy's `pedantic` + `nursery` groups catch more issues than ESLint + Flow.

### 4.6 Error Reporting and Diagnostics

**Old client:**
- Sentry integration (`@sentry/electron`) for production error reporting.
- Structured logging via Winston with daily rotation.
- Log upload to support server (`desktop-upload.cozycloud.cc`).
- Support email with debug info via Cozy's sendmail.
- Performance profiling via `measureTime()` (controlled by `MEASURE_PERF`).
- Debug signal: USR1 to list watched paths.

**New client:**
- Structured JSONL logging with `tracing` + `tracing-subscriber`.
- Daily rotation via `tracing-appender`.
- Dual output: human-readable stderr + JSONL files.
- No Sentry, no log upload, no support tools, no performance profiling.

**Gaps to address:**
- Consider adding Sentry or a similar error reporting service.
- Add a log upload mechanism for support.
- Add performance profiling (at least timing for sync cycles, downloads,
  uploads).
- Consider adding a debug signal handler (e.g., SIGUSR1) to dump internal
  state.

### 4.7 Test Infrastructure Comparison

| Aspect | Old Client | New Client |
|--------|-----------|------------|
| Unit tests | Mocha + chai + should | `#[test]` + assertions |
| Integration tests | 16 files against cozy-stack | 1 file, 9 tests (gated) |
| Scenario tests | 69 scenarios, capture-replay | None |
| Property tests | jsverify, 20 recorded scenarios | proptest, 6 property tests |
| Regression tests | 2 specific bug tests | proptest-regressions file |
| Simulation | Two-client simulation | Single-client simulator |
| Coverage | Istanbul (reported broken) | None |
| Mocking | sinon, PouchDB in-memory | wiremock, MockRemote, MockFs |
| Conformance | None | 9 changes feed conformance tests |

**New client strengths:**
- The property-based simulator is more systematic than the old client's
  approach, with explicit convergence, idempotency, and store consistency
  invariants.
- The changes feed conformance tests (comparing mock vs. real cozy-stack)
  are unique to the new client and prevent mock drift.
- The MockRemote faithfully reproduces cozy-stack behavior including trash
  cascades and changes feed ordering.

**New client gaps:**
- No scenario-based tests with real filesystem events.
- No multi-client simulation.
- No code coverage tracking.
- Fewer integration tests against real cozy-stack.

### 4.8 i18n / Translations

**Old client:** 12 languages via Transifex.

**New client:** No i18n (CLI-only, English messages hardcoded).
Not a priority until a GUI is built.

### 4.9 Configuration Best Practices

**Old client:**
- Validates sync path is not home directory or system root.
- Hidden tmp directory on Windows.
- Atomic config writes via temp file + rename.
- Config migration between versions.

**New client:**
- Atomic config writes via temp file + rename with `0o600` permissions. Good.
- No sync path validation (could sync `/` or `$HOME`).
- No config migration.

**Recommendation:** Add basic sync path validation to prevent users from
accidentally syncing their entire home directory.

### 4.10 Environment Variables

The old client uses many environment variables for development and testing.
Worth adopting for the new client:

| Variable | Purpose | Priority |
|----------|---------|----------|
| `RUST_LOG` | Already supported | Done |
| `COZY_DESKTOP_HEARTBEAT` | Configurable polling interval | High |
| `DEBUG` | Verbose debug output | Medium |
| `COZY_NO_SENTRY` | Disable error reporting | Low |
| `COZY_DESKTOP_DIR` | Override sync directory | Medium |
| `COZY_STACK_TOKEN` | Token for CI testing | High |

### 4.11 Security Considerations

**Old client:**
- OAuth secrets stored in config file.
- PBKDF2 passphrase hashing for automated registration.
- Code signing on all platforms.
- Hardened runtime on macOS.
- `sanitizeName: false` to prevent cozy-client from altering filenames.

**New client:**
- OAuth secrets stored in config with `0o600` permissions.
- `Debug` impl redacts all secrets (auth.rs line 56-73). Good.
- No code signing or hardened runtime (not yet packaged).

### 4.12 Cozy-Stack API Gaps

API calls in the old client that are missing from the new client:

| API | Old Client | New Client | Priority |
|-----|-----------|------------|----------|
| Changes feed | Yes | Yes | Done |
| File download | Yes | Yes | Done |
| File upload | Yes | Yes | Done |
| File update | Yes | Yes | Done |
| Dir creation | Yes | Yes | Done |
| Trash | Yes | Yes | Done |
| Move/rename | Yes | Yes | Done |
| Disk usage | Yes | No | Medium |
| Warnings (402) | Yes | No | Medium |
| Capabilities | Yes | No | Low |
| Feature flags | Yes | No | Low |
| Old file versions | Yes | No | Medium |
| Selective sync attrs | Yes | No | Medium |
| Mango queries | Yes | No | Low |
| Sendmail job | Yes | No | Low |
