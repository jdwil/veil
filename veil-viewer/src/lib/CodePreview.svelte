<script lang="ts">
  import { onMount } from 'svelte';
  import { generatedCode } from '$lib/store';
  import { highlightLine } from '$lib/rustHighlight';

  let files = $state<Record<string, string>>({});
  let selectedFile = $state<string | null>(null);
  let loading = $state(true);
  let visible = $state(false);

  // Pick a sensible default file the first time files arrive, preserving the
  // user's selection across live updates when it still exists.
  function chooseSelection(paths: string[]) {
    if (selectedFile && paths.includes(selectedFile)) return;
    // Prefer a target-appropriate default, not hardcoded Rust application/mod.rs
    selectedFile =
      paths.find((p) => p.endsWith('mod.rs') && p.includes('application')) ||
      paths.find((p) => p.endsWith('.rs')) ||
      paths.find((p) => p.endsWith('.ts') || p.endsWith('.svelte')) ||
      paths[0] ||
      null;
  }

  /** Apply a generated-code map without creating a reactive read/write cycle. */
  function applyGenerated(code: Record<string, string> | null | undefined) {
    if (!code) return;
    files = code;
    chooseSelection(Object.keys(code).sort());
    loading = false;
  }

  onMount(() => {
    // Live updates from the store (edits / file switch) — subscribe, not $effect.
    // An $effect that writes `files`/`selectedFile` while reading them loops
    // (Svelte effect_update_depth_exceeded).
    let hadStoreValue = false;
    const unsub = generatedCode.subscribe((code) => {
      if (code) {
        hadStoreValue = true;
        applyGenerated(code);
      }
    });

    // Initial fetch if store has not been populated yet
    if (!hadStoreValue) {
      void fetch('http://localhost:3001/api/generated')
        .then(async (res) => {
          if (res.ok) applyGenerated(await res.json());
          else loading = false;
        })
        .catch((e) => {
          console.error('Failed to fetch generated code:', e);
          loading = false;
        });
    }

    return unsub;
  });

  function toggle() {
    visible = !visible;
  }

  // UX-028: all generated files for the target, not only .rs
  let sortedPaths = $derived(
    Object.keys(files)
      .filter((p) => !p.endsWith('.map'))
      .sort()
  );
  let content = $derived(selectedFile ? files[selectedFile] || '' : '');
  // Highlighted lines (Rust highlighter is best-effort for other langs too).
  let lines = $derived(content ? content.split('\n').map(highlightLine) : []);
</script>

<div class="code-preview">
  <button class="toggle-btn" onclick={toggle} title="Secondary: generated target source">
    {visible ? '◀ Hide preview' : '▶ Source preview'}
  </button>

  {#if visible}
    <div class="panel">
      <div class="file-list">
        {#each sortedPaths as path}
          <button
            class="file-item"
            class:active={selectedFile === path}
            onclick={() => selectedFile = path}
          >
            {path.split('/').slice(-2).join('/')}
          </button>
        {/each}
      </div>
      <div class="code-content">
        {#if loading}
          <p class="loading">Loading...</p>
        {:else if content}
          <pre><code>{#each lines as toks, i}<span class="ln">{#each toks as t}<span class={t.cls}>{t.text}</span>{/each}
</span>{/each}</code></pre>
        {:else}
          <p class="empty">Select a file</p>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .code-preview {
    position: fixed;
    right: 0;
    top: 60px;
    /* Leave room for bottom ReviewDock so the vertical toggle doesn't steal clicks */
    bottom: max(140px, 22vh);
    z-index: 10;
    display: flex;
    flex-direction: row;
    align-items: flex-start;
    pointer-events: none;
  }
  .code-preview > * {
    pointer-events: auto;
  }

  .toggle-btn {
    background: var(--veil-surface);
    color: var(--veil-text);
    border: 1px solid var(--veil-border);
    border-radius: 4px 0 0 4px;
    padding: 8px 12px;
    cursor: pointer;
    font-size: 12px;
    writing-mode: vertical-rl;
    text-orientation: mixed;
    margin-top: 20px;
  }

  .toggle-btn:hover {
    background: var(--veil-border);
  }

  .panel {
    width: 500px;
    height: 100%;
    background: var(--veil-bg);
    border-left: 1px solid var(--veil-border);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .file-list {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 8px;
    border-bottom: 1px solid var(--veil-border);
    max-height: 120px;
    overflow-y: auto;
  }

  .file-item {
    background: var(--veil-surface);
    color: var(--veil-text-secondary);
    border: 1px solid var(--veil-border);
    border-radius: 3px;
    padding: 3px 8px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
  }

  .file-item:hover {
    background: var(--veil-border);
  }

  .file-item.active {
    background: var(--veil-accent);
    color: var(--veil-bg);
    border-color: var(--veil-accent);
  }

  .code-content {
    flex: 1;
    overflow: auto;
    padding: 12px;
  }

  .code-content pre {
    margin: 0;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 12px;
    line-height: 1.5;
    color: var(--veil-text);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .loading, .empty {
    color: var(--veil-text-dim);
    font-size: 13px;
    padding: 20px;
  }

  /* Rust syntax highlighting (tokens from rustHighlight.ts). */
  .code-content :global(.tok-keyword)  { color: #c792ea; }
  .code-content :global(.tok-type)     { color: #82aaff; }
  .code-content :global(.tok-string)   { color: #c3e88d; }
  .code-content :global(.tok-number)   { color: #f78c6c; }
  .code-content :global(.tok-comment)  { color: #546e7a; font-style: italic; }
  .code-content :global(.tok-fn)       { color: #82b1ff; }
  .code-content :global(.tok-macro)    { color: #ffcb6b; }
  .code-content :global(.tok-attr)     { color: #ffcb6b; }
  .code-content :global(.tok-lifetime) { color: #f78c6c; }

  /* Light mode syntax highlighting */
  :global([data-theme="light"]) .code-content :global(.tok-keyword)  { color: #7c3aed; }
  :global([data-theme="light"]) .code-content :global(.tok-type)     { color: #2563eb; }
  :global([data-theme="light"]) .code-content :global(.tok-string)   { color: #16a34a; }
  :global([data-theme="light"]) .code-content :global(.tok-number)   { color: #c2410c; }
  :global([data-theme="light"]) .code-content :global(.tok-comment)  { color: #6b7280; font-style: italic; }
  :global([data-theme="light"]) .code-content :global(.tok-fn)       { color: #1d4ed8; }
  :global([data-theme="light"]) .code-content :global(.tok-macro)    { color: #b45309; }
  :global([data-theme="light"]) .code-content :global(.tok-attr)     { color: #b45309; }
  :global([data-theme="light"]) .code-content :global(.tok-lifetime) { color: #c2410c; }
</style>
