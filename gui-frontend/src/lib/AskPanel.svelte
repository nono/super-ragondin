<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { marked } from 'marked'
  import { commands, events } from '../bindings'

  type PanelState = 'loading' | 'no-api-key' | 'idle' | 'asking' | 'clarifying' | 'done' | 'error'

  let state: PanelState = $state('loading')
  let suggestions: string[] = $state([])
  let question: string = $state('')
  let lastQuestion: string = $state('')
  let answer: string = $state('')
  let errorMessage: string = $state('')
  let clarifyQuestion: string = $state('')
  let clarifyChoices: string[] = $state([])
  let clarifyInput: string = $state('')
  let apiKeyInput: string = $state('')
  let savingKey: boolean = $state(false)
  let keyError: string | null = $state(null)

  let unlistenAskUser: (() => void) | undefined

  onMount(async () => {
    unlistenAskUser = await events.askUserEvent.listen((event) => {
      clarifyQuestion = event.payload.question
      clarifyChoices = event.payload.choices
      clarifyInput = ''
      state = 'clarifying'
    })
    await loadSuggestions()
  })

  onDestroy(() => {
    unlistenAskUser?.()
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

  function friendlyError(err: string): string {
    if (err === 'NoApiKey') return 'No OpenRouter API key configured.'
    if (err === 'NoFilesIndexed') return 'No files indexed yet — waiting for first sync.'
    return `Something went wrong: ${err}`
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      ask(question)
    }
  }

  async function sendClarification(answer: string) {
    if (!answer.trim()) return
    state = 'asking'
    const result = await commands.answerUser(answer)
    if (result.status === 'error') {
      errorMessage = result.error
      state = 'error'
    }
  }

  function handleClarifyKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void sendClarification(clarifyInput)
    }
  }

  async function saveApiKey() {
    if (!apiKeyInput.trim()) return
    savingKey = true
    keyError = null
    try {
      const result = await commands.setApiKey(apiKeyInput)
      if (result.status === 'ok') {
        apiKeyInput = ''
        await loadSuggestions()
      } else {
        keyError = result.error
      }
    } finally {
      savingKey = false
    }
  }

  function handleApiKeyKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault()
      void saveApiKey()
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
        <p class="banner-label">No OpenRouter API key configured.</p>
        <div class="key-form">
          <input
            type="password"
            bind:value={apiKeyInput}
            placeholder="sk-or-…"
            disabled={savingKey}
            onkeydown={handleApiKeyKeydown}
            oninput={() => { if (keyError) keyError = null }}
          />
          <button
            class="save-key-btn"
            onclick={saveApiKey}
            disabled={savingKey || !apiKeyInput.trim()}
          >
            {savingKey ? 'Saving…' : 'Save'}
          </button>
        </div>
        {#if keyError}
          <p class="key-error">{keyError}</p>
        {/if}
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

    {:else if state === 'clarifying'}
      <div class="message user">{lastQuestion}</div>
      <div class="clarify-box">
        <p class="clarify-question">{clarifyQuestion}</p>
        <ul class="chips">
          {#each clarifyChoices as choice, i}
            <li>
              <button class="chip" onclick={() => void sendClarification(choice)}>
                <span class="chip-arrow">{i + 1}.</span> {choice}
              </button>
            </li>
          {/each}
        </ul>
        <div class="clarify-input-row">
          <input
            type="text"
            bind:value={clarifyInput}
            placeholder="Or type a custom answer…"
            onkeydown={handleClarifyKeydown}
          />
          <button
            class="send-btn"
            onclick={() => void sendClarification(clarifyInput)}
            disabled={!clarifyInput.trim()}
          >
            Send
          </button>
        </div>
      </div>

    {:else if state === 'done'}
      <div class="message user">{lastQuestion}</div>
      <div class="message assistant markdown">{@html marked(answer)}</div>

    {:else if state === 'error'}
      <div class="message user">{lastQuestion}</div>
      <div class="message error-msg">{friendlyError(errorMessage)}</div>
    {/if}
  </div>

  {#if state !== 'no-api-key'}
    <div class="input-row">
      <input
        type="text"
        bind:value={question}
        placeholder={state === 'done' || state === 'error' ? 'Ask another question…' : 'Ask anything about your files…'}
        disabled={state === 'asking' || state === 'clarifying' || state === 'loading'}
        onkeydown={handleKeydown}
      />
      <button
        class="send-btn"
        onclick={() => ask(question)}
        disabled={state === 'asking' || state === 'clarifying' || state === 'loading' || !question.trim()}
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
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .banner-label {
    margin: 0;
  }
  .key-form {
    display: flex;
    gap: 6px;
  }
  .key-form input {
    flex: 1;
    padding: 6px 8px;
    border: 1px solid #ffcc80;
    border-radius: 4px;
    font-size: 12px;
    background: #fff;
    color: #333;
    outline: none;
    font-family: inherit;
  }
  .key-form input:focus {
    border-color: #e65100;
  }
  .key-form input:disabled {
    opacity: 0.5;
  }
  .save-key-btn {
    padding: 6px 10px;
    background: #e65100;
    color: #fff;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .save-key-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .key-error {
    margin: 0;
    color: #c62828;
    font-size: 11px;
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
  .message.markdown :global(p) { margin: 0 0 8px; }
  .message.markdown :global(p:last-child) { margin-bottom: 0; }
  .message.markdown :global(h1),
  .message.markdown :global(h2),
  .message.markdown :global(h3) { font-size: 13px; font-weight: 700; margin: 10px 0 4px; }
  .message.markdown :global(ul),
  .message.markdown :global(ol) { margin: 4px 0 8px; padding-left: 20px; }
  .message.markdown :global(li) { margin-bottom: 2px; }
  .message.markdown :global(code) {
    background: #e8e8e3;
    border-radius: 3px;
    padding: 1px 4px;
    font-size: 11px;
    font-family: monospace;
  }
  .message.markdown :global(pre) {
    background: #e8e8e3;
    border-radius: 4px;
    padding: 8px 10px;
    overflow-x: auto;
    margin: 6px 0;
  }
  .message.markdown :global(pre code) { background: none; padding: 0; }
  .message.markdown :global(strong) { font-weight: 700; }
  .message.markdown :global(em) { font-style: italic; }
  .message.markdown :global(a) { color: #2f80ed; text-decoration: underline; }

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

  .clarify-box {
    background: #f7f7f3;
    border: 1px solid #e0e0d8;
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .clarify-question {
    font-size: 12px;
    color: #333;
    font-weight: 500;
    margin: 0;
  }
  .clarify-input-row {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }
  .clarify-input-row input {
    flex: 1;
    padding: 6px 10px;
    border: 1px solid #ddd;
    border-radius: 6px;
    font-size: 12px;
    color: #333;
    background: #fafafa;
    outline: none;
    font-family: inherit;
  }
  .clarify-input-row input:focus { border-color: #2f80ed; }
</style>
