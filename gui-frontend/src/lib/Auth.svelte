<script>
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'

  export let error = null

  onMount(() => {
    invoke('start_auth')
  })

  function retry() {
    error = null
    invoke('start_auth')
  }
</script>

<div class="container">
  <div class="icon">🔑</div>
  <h1>Connecting to Cozy</h1>
  {#if error}
    <p class="error">{error}</p>
    <button on:click={retry}>Retry</button>
  {:else}
    <p>A browser window has been opened.</p>
    <p>Sign in to your Cozy and grant access.</p>
    <p class="hint">Waiting for authorization…</p>
  {/if}
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
  p {
    color: #aaa;
    font-size: 13px;
    line-height: 1.5;
  }
  .hint {
    color: #666;
    font-size: 12px;
  }
  .error {
    color: #f66;
  }
  button {
    background: #4fc;
    color: #111;
    border: none;
    border-radius: 4px;
    padding: 8px 16px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    margin-top: 8px;
  }
</style>
