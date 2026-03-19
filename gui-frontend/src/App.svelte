<script>
  import { onMount, onDestroy } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import Setup from './lib/Setup.svelte'
  import Auth from './lib/Auth.svelte'
  import Syncing from './lib/Syncing.svelte'

  // String values match the Rust AppState enum serialized with #[serde(rename_all = "PascalCase")].
  // See crates/gui/src/commands.rs — AppState enum.
  // null = loading; 'Unconfigured' | 'Unauthenticated' | 'Ready'
  let appState = null
  let authError = null

  let unlistenAuthComplete
  let unlistenAuthError

  onMount(async () => {
    unlistenAuthComplete = await listen('auth_complete', () => {
      appState = 'Ready'
      authError = null
    })
    unlistenAuthError = await listen('auth_error', (event) => {
      authError = event.payload.message
    })
    appState = await invoke('get_app_state')
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

<main>
  {#if appState === null}
    <div class="loading">Loading…</div>
  {:else if appState === 'Unconfigured'}
    <Setup on:complete={handleSetupComplete} />
  {:else if appState === 'Unauthenticated'}
    <Auth error={authError} />
  {:else if appState === 'Ready'}
    <Syncing />
  {/if}
</main>

<style>
  :global(*, *::before, *::after) {
    box-sizing: border-box;
    margin: 0;
    padding: 0;
  }
  :global(body) {
    font-family: system-ui, sans-serif;
    font-size: 14px;
    background: #1a1a1a;
    color: #e0e0e0;
  }
  main {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .loading {
    color: #888;
  }
</style>
