<script lang="ts">
  import { onMount } from 'svelte';

  let files = $state<Record<string, string>>({});
  let selectedFile = $state<string | null>(null);
  let loading = $state(true);
  let visible = $state(false);

  onMount(async () => {
    try {
      const res = await fetch('http://localhost:3001/api/generated');
      if (res.ok) {
        files = await res.json();
        const paths = Object.keys(files).sort();
        if (paths.length > 0) {
          // Default to first application/mod.rs
          selectedFile = paths.find(p => p.includes('application/mod.rs')) || paths[0];
        }
      }
    } catch (e) {
      console.error('Failed to fetch generated code:', e);
    } finally {
      loading = false;
    }
  });

  function toggle() {
    visible = !visible;
  }

  let sortedPaths = $derived(Object.keys(files).filter(p => p.endsWith('.rs')).sort());
  let content = $derived(selectedFile ? files[selectedFile] || '' : '');
</script>

<div class="code-preview">
  <button class="toggle-btn" onclick={toggle}>
    {visible ? '◀ Hide Code' : '▶ Generated Rust'}
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
          <pre><code>{content}</code></pre>
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
    bottom: 0;
    z-index: 100;
    display: flex;
    flex-direction: row;
    align-items: flex-start;
  }

  .toggle-btn {
    background: #1e293b;
    color: #e2e8f0;
    border: 1px solid #334155;
    border-radius: 4px 0 0 4px;
    padding: 8px 12px;
    cursor: pointer;
    font-size: 12px;
    writing-mode: vertical-rl;
    text-orientation: mixed;
    margin-top: 20px;
  }

  .toggle-btn:hover {
    background: #334155;
  }

  .panel {
    width: 500px;
    height: 100%;
    background: #0f172a;
    border-left: 1px solid #334155;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .file-list {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 8px;
    border-bottom: 1px solid #334155;
    max-height: 120px;
    overflow-y: auto;
  }

  .file-item {
    background: #1e293b;
    color: #94a3b8;
    border: 1px solid #334155;
    border-radius: 3px;
    padding: 3px 8px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
  }

  .file-item:hover {
    background: #334155;
  }

  .file-item.active {
    background: #1d4ed8;
    color: #fff;
    border-color: #3b82f6;
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
    color: #e2e8f0;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .loading, .empty {
    color: #64748b;
    font-size: 13px;
    padding: 20px;
  }
</style>
