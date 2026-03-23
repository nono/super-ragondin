<script lang="ts">
  import { onMount } from 'svelte'
  import { commands } from '../bindings'

  type PanelState = 'loading' | 'no-api-key' | 'idle' | 'asking' | 'done' | 'error'

  let state: PanelState = $state('loading')
  let suggestions: string[] = $state([])
  let question: string = $state('')
  let lastQuestion: string = $state('')
  let answer: string = $state('')
  let errorMessage: string = $state('')

  onMount(async () => {
    await loadSuggestions()
  })

  async function loadSuggestions() {
    state = 'loading'
    const result = await commands.getSuggestions()
    if (result.status === 'ok') {
      suggestions = result.data
      state = 'idle'
    } else if (result.error === 'NoApiKey') {
      state = 'no-api-key'
    } else if (result.error === 'NoFilesIndexed') {
      suggestions = []
      state = 'idle'
    } else {
      suggestions = []
      state = 'idle'
    }
  }

  async function ask(q: string) {
    if (!q.trim()) return
    lastQuestion = q
    question = ''
    state = 'asking'
    const result = await commands.askQuestion(q)
    if (result.status === 'ok') {
      answer = result.data
      state = 'done'
    } else {
      errorMessage = result.error
      state = 'error'
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      ask(question)
    }
  }
</script>

<div class="panel">
  <div class="panel-header">
    <span class="icon">✦</span>
    <span class="title">Ask</span>
  </div>

  <div class="panel-body">
    {#if state === 'loading'}
      <p class="hint">Loading suggestions…</p>

    {:else if state === 'no-api-key'}
      <div class="banner">
        Add your OpenRouter API key during setup to use the assistant.
      </div>

    {:else if state === 'idle'}
      {#if suggestions.length > 0}
        <p class="hint">Not sure what to ask? Here are some ideas:</p>
        <ul class="chips">
          {#each suggestions as s}
            <li>
              <button class="chip" onclick={() => ask(s)}>
                <span class="chip-arrow">↗</span> {s}
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="hint">No files indexed yet — waiting for first sync.</p>
      {/if}

    {:else if state === 'asking'}
      <div class="message user">{lastQuestion}</div>
      <div class="thinking">
        <span class="dot"></span><span class="dot"></span><span class="dot"></span>
        Thinking…
      </div>

    {:else if state === 'done'}
      <div class="message user">{lastQuestion}</div>
      <div class="message assistant">{answer}</div>

    {:else if state === 'error'}
      <div class="message user">{lastQuestion}</div>
      <div class="message error-msg">{errorMessage}</div>
    {/if}
  </div>

  {#if state !== 'no-api-key'}
    <div class="input-row">
      <input
        type="text"
        bind:value={question}
        placeholder={state === 'done' || state === 'error' ? 'Ask another question…' : 'Ask anything about your files…'}
        disabled={state === 'asking' || state === 'loading'}
        onkeydown={handleKeydown}
      />
      <button
        class="send-btn"
        onclick={() => ask(question)}
        disabled={state === 'asking' || state === 'loading' || !question.trim()}
      >
        Ask
      </button>
    </div>
  {/if}
</div>

<style>
  .panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    background: #fff;
    min-width: 0;
  }
  .panel-header {
    padding: 12px 16px 10px;
    border-bottom: 1px solid #ebebeb;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .icon { font-size: 14px; }
  .title { font-size: 13px; font-weight: 600; color: #333; }

  .panel-body {
    flex: 1;
    padding: 16px 18px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .hint { font-size: 12px; color: #888; }

  .banner {
    background: #fff3e0;
    border: 1px solid #ffe0b2;
    border-radius: 6px;
    padding: 10px 14px;
    font-size: 12px;
    color: #e65100;
  }

  .chips {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .chip {
    width: 100%;
    text-align: left;
    padding: 9px 13px;
    background: #f7f7f3;
    border: 1px solid #e0e0d8;
    border-radius: 8px;
    font-size: 12px;
    color: #444;
    cursor: pointer;
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .chip:hover { background: #eeeee8; }
  .chip-arrow { color: #aaa; font-size: 11px; flex-shrink: 0; }

  .message {
    padding: 9px 13px;
    border-radius: 8px;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .message.user {
    background: #2f80ed;
    color: #fff;
    align-self: flex-end;
    border-bottom-right-radius: 3px;
    max-width: 88%;
  }
  .message.assistant {
    background: #f3f3ee;
    color: #333;
    border: 1px solid #e8e8e3;
    border-bottom-left-radius: 3px;
    align-self: flex-start;
    max-width: 95%;
  }
  .message.error-msg {
    background: #fff0f0;
    color: #c62828;
    border: 1px solid #ffcdd2;
    border-bottom-left-radius: 3px;
    align-self: flex-start;
  }

  .thinking {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: #aaa;
    padding: 8px 0;
  }
  .thinking .dot {
    width: 5px;
    height: 5px;
    background: #ccc;
    border-radius: 50%;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .thinking .dot:nth-child(2) { animation-delay: 0.2s; }
  .thinking .dot:nth-child(3) { animation-delay: 0.4s; }
  @keyframes pulse {
    0%, 80%, 100% { opacity: 0.3; transform: scale(0.8); }
    40% { opacity: 1; transform: scale(1); }
  }

  .input-row {
    padding: 10px 14px;
    border-top: 1px solid #ebebeb;
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .input-row input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 12px;
    color: #333;
    background: #fafafa;
    outline: none;
    font-family: inherit;
  }
  .input-row input:focus { border-color: #2f80ed; }
  .input-row input::placeholder { color: #bbb; }
  .input-row input:disabled { opacity: 0.5; }
  .send-btn {
    padding: 8px 14px;
    background: #2f80ed;
    color: #fff;
    border: none;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }
  .send-btn:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
