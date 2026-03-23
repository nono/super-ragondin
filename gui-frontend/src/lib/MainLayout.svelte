<script lang="ts">
  import SyncPanel from './SyncPanel.svelte'
  import AskPanel from './AskPanel.svelte'
</script>

<div class="layout">
  <SyncPanel />
  <AskPanel />
</div>

<style>
  .layout {
    display: flex;
    width: 100%;
    height: 100vh;
    overflow: hidden;
  }
</style>
