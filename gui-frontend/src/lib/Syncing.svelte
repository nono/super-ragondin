<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from '../bindings'
  import type { SyncState } from '../bindings'

  let status: SyncState = $state('Idle')
  let lastSync: string | null = $state(null)

  let unlistenSyncStatus: (() => void) | undefined

  onMount(async () => {
    unlistenSyncStatus = await events.syncStatusEvent.listen((event) => {
      status = event.payload.status
      lastSync = event.payload.last_sync
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
