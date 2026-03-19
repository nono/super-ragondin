<script>
  import { onMount, onDestroy } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'

  // status values match the Rust SyncState enum serialized with #[serde(rename_all = "PascalCase")].
  // See crates/gui/src/commands.rs — SyncState enum.
  let status = 'Idle'
  let lastSync = null

  let unlistenSyncStatus

  onMount(async () => {
    unlistenSyncStatus = await listen('sync_status', (event) => {
      status = event.payload.status
      lastSync = event.payload.last_sync
    })
    invoke('start_sync')
  })

  onDestroy(() => {
    unlistenSyncStatus?.()
  })

  function formatLastSync(iso) {
    if (!iso) return 'Never'
    try {
      return new Date(iso).toLocaleString()
    } catch {
      return iso
    }
  }
</script>

<div class="container">
  <div class="icon">☁️</div>
  <h1>Synchronizing</h1>
  <p class="status">{status === 'Syncing' ? 'Syncing…' : 'Up to date'}</p>
  <p class="hint">Last sync: {formatLastSync(lastSync)}</p>
</div>

<style>
  .container {
    width: 360px;
    padding: 24px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .icon {
    font-size: 36px;
    margin-bottom: 4px;
  }
  h1 {
    font-size: 18px;
  }
  .status {
    color: #4fc;
    font-size: 14px;
  }
  .hint {
    color: #666;
    font-size: 12px;
  }
</style>
