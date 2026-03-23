<script lang="ts">
  import { commands } from '../bindings'

  interface Props {
    oncomplete: () => void
  }

  const { oncomplete }: Props = $props()

  let instanceUrl: string = $state('')
  let syncDir: string = $state('')
  let apiKey: string = $state('')
  let error: string | null = $state(null)
  let submitting: boolean = $state(false)

  async function handleSubmit() {
    submitting = true
    error = null
    try {
      const result = await commands.initConfig(instanceUrl, syncDir, apiKey || null)
      if (result.status === 'error') {
        error = result.error
      } else {
        oncomplete()
      }
    } catch (e) {
      error = String(e)
    } finally {
      submitting = false
    }
  }
</script>

<div class="container">
  <h1>Super Ragondin</h1>
  <form onsubmit={(e) => { e.preventDefault(); handleSubmit() }}>
    <label>
      Cozy instance URL
      <input
        type="url"
        bind:value={instanceUrl}
        placeholder="https://alice.mycozy.cloud"
        required
      />
    </label>
    <label>
      Sync directory
      <input
        type="text"
        bind:value={syncDir}
        placeholder="/home/user/Cozy"
        required
      />
    </label>
    <label>
      OpenRouter API key
      <input
        type="password"
        bind:value={apiKey}
        placeholder="sk-or-…"
      />
    </label>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <button type="submit" disabled={submitting}>
      {submitting ? 'Saving…' : 'Connect to Cozy →'}
    </button>
  </form>
</div>

<style>
  .container {
    width: 380px;
    padding: 24px;
  }
  h1 {
    font-size: 18px;
    margin-bottom: 20px;
    text-align: center;
    color: #333;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: #666;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  input {
    background: #fff;
    border: 1px solid #ccc;
    border-radius: 4px;
    padding: 8px 10px;
    color: #333;
    font-size: 14px;
  }
  input:focus {
    outline: none;
    border-color: #2f80ed;
  }
  button {
    background: #2f80ed;
    color: #fff;
    border: none;
    border-radius: 4px;
    padding: 10px;
    font-size: 14px;
    font-weight: 600;
    cursor: pointer;
    margin-top: 4px;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .error {
    color: #d32f2f;
    font-size: 13px;
  }
</style>
