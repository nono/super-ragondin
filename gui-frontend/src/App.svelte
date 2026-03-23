<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { commands, events } from './bindings'
  import type { AppState } from './bindings'
  import Setup from './lib/Setup.svelte'
  import Auth from './lib/Auth.svelte'
  import MainLayout from './lib/MainLayout.svelte'

  let appState: AppState | null = $state(null)
  let authError: string | null = $state(null)

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

<main>
  {#if appState === null}
    <div class="loading">Loading…</div>
  {:else if appState === 'Unconfigured'}
    <Setup oncomplete={handleSetupComplete} />
  {:else if appState === 'Unauthenticated'}
    <Auth bind:error={authError} />
  {:else if appState === 'Ready'}
    <MainLayout />
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
    background: #f5f5f0;
    color: #333;
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
