<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from '../bindings'
  import type { SyncState } from '../bindings'

  let status: SyncState = $state('Idle')
  let lastSync: string | null = $state(null)
  let recentFiles: string[] = $state([])
  let version: string = $state('')

  let unlistenSyncStatus: (() => void) | undefined

  async function refreshRecentFiles() {
    const result = await commands.getRecentFiles()
    if (result.status === 'ok') {
      recentFiles = result.data
    }
    // On error: silently keep existing list
  }

  onMount(async () => {
    unlistenSyncStatus = await events.syncStatusEvent.listen(async (event) => {
      status = event.payload.status
      lastSync = event.payload.last_sync
      if (status === 'Idle') {
        await refreshRecentFiles()
      }
    })
    commands.startSync()
    const v = await commands.getVersion()
    version = v
    await refreshRecentFiles()
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

  function fileIcon(path: string): string {
    const ext = path.split('.').pop()?.toLowerCase() ?? ''
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg'].includes(ext)) return '🖼'
    if (['pdf'].includes(ext)) return '📕'
    if (['md', 'txt', 'csv'].includes(ext)) return '📄'
    return '📝'
  }

  function fileName(path: string): string {
    return path.split('/').pop() ?? path
  }
</script>

<div class="panel">
  <div class="app-title">Super Ragondin</div>
  <p class="version">v{version}</p>

  <div class="status-badge" class:syncing={status === 'Syncing'}>
    <span class="dot"></span>
    <span class="label">{status === 'Syncing' ? 'Syncing…' : 'Up to date'}</span>
  </div>

  <div class="section">
    <div class="section-title">Recent files</div>
    {#if recentFiles.length === 0}
      <p class="empty">No files yet</p>
    {:else}
      <ul class="file-list">
        {#each recentFiles as file}
          <li class="file-item">
            <span class="file-icon">{fileIcon(file)}</span>
            <span class="file-name" title={file}>{fileName(file)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <p class="last-sync">Last sync: {formatLastSync(lastSync)}</p>
</div>

<style>
  .panel {
    width: 220px;
    flex-shrink: 0;
    background: #efefea;
    border-right: 1px solid #ddd;
    display: flex;
    flex-direction: column;
    padding: 16px 14px;
    gap: 12px;
    overflow-y: auto;
  }
  .app-title {
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: #888;
    text-transform: uppercase;
  }
  .version {
    font-size: 10px;
    color: #aaa;
  }
  .status-badge {
    display: flex;
    align-items: center;
    gap: 7px;
    background: #e4f4e4;
    border: 1px solid #b5dbb5;
    border-radius: 6px;
    padding: 7px 10px;
  }
  .status-badge.syncing {
    background: #fff4e0;
    border-color: #f0c060;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4caf50;
    flex-shrink: 0;
  }
  .status-badge.syncing .dot {
    background: #f0a020;
  }
  .label {
    font-size: 12px;
    color: #2e7d2e;
    font-weight: 600;
  }
  .status-badge.syncing .label {
    color: #a06010;
  }
  .section-title {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: #aaa;
    margin-bottom: 6px;
  }
  .file-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .file-item {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 7px;
    border-radius: 5px;
    background: #fff;
    border: 1px solid #e8e8e3;
  }
  .file-icon {
    font-size: 12px;
    flex-shrink: 0;
  }
  .file-name {
    font-size: 11px;
    color: #444;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    font-size: 11px;
    color: #aaa;
  }
  .last-sync {
    font-size: 10px;
    color: #aaa;
    margin-top: auto;
  }
</style>
