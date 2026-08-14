<script lang="ts">
  /**
   * Body / step source with view ↔ edit modes.
   *
   * - **View** (default): plain formatted VEIL source (read-only). Highlighting later.
   * - **Edit**: structured BlockEditor (inputs / selects / buttons).
   *
   * Preference stored in localStorage so refresh keeps the mode.
   */
  import BlockEditor from './BlockEditor.svelte';
  import { exprToVeil } from './expr-serialize';
  import type { Expr } from './expr-types';

  interface Props {
    exprs: Expr[];
    onChange: (exprs: Expr[]) => void;
    /** Optional depth for nested BlockEditor */
    depth?: number;
    emptyLabel?: string;
  }

  let {
    exprs,
    onChange,
    depth = 0,
    emptyLabel = 'No body expressions.',
  }: Props = $props();

  const MODE_KEY = 'veil.bodySource.mode';

  type Mode = 'view' | 'edit';

  function loadMode(): Mode {
    if (typeof localStorage === 'undefined') return 'view';
    const v = localStorage.getItem(MODE_KEY);
    return v === 'edit' ? 'edit' : 'view';
  }

  let mode = $state<Mode>(typeof window !== 'undefined' ? loadMode() : 'view');

  function setMode(next: Mode) {
    mode = next;
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      /* ignore */
    }
  }

  let sourceText = $derived.by(() => {
    if (!exprs.length) return '';
    return exprs.map((e) => exprToVeil(e, 0)).join('\n');
  });
</script>

<div class="body-source" data-mode={mode}>
  <div class="body-source-toolbar" role="toolbar" aria-label="Body display mode">
    <div class="mode-toggle" role="group" aria-label="View or edit body">
      <button
        type="button"
        class="mode-btn"
        class:active={mode === 'view'}
        aria-pressed={mode === 'view'}
        onclick={() => setMode('view')}
        title="Read-only source"
      >
        View
      </button>
      <button
        type="button"
        class="mode-btn"
        class:active={mode === 'edit'}
        aria-pressed={mode === 'edit'}
        onclick={() => setMode('edit')}
        title="Structured editor"
      >
        Edit
      </button>
    </div>
  </div>

  {#if mode === 'view'}
    {#if sourceText}
      <pre class="body-source-view" aria-label="Body source (view mode)"><code>{sourceText}</code></pre>
    {:else}
      <p class="body-source-empty">{emptyLabel}</p>
    {/if}
  {:else if exprs.length > 0}
    <div class="body-source-edit">
      <BlockEditor {exprs} {onChange} {depth} />
    </div>
  {:else}
    <div class="body-source-edit">
      <BlockEditor exprs={[]} {onChange} {depth} />
    </div>
  {/if}
</div>

<style>
  .body-source {
    display: flex;
    flex-direction: column;
    min-height: 0;
    gap: 0;
  }

  .body-source-toolbar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding: 0 0 6px;
    flex-shrink: 0;
  }

  .mode-toggle {
    display: inline-flex;
    border: 1px solid var(--veil-border, #2e2e2e);
    border-radius: 6px;
    overflow: hidden;
    background: var(--veil-surface-alt, rgba(26, 26, 26, 0.9));
  }

  .mode-btn {
    border: none;
    background: transparent;
    color: var(--veil-text-dim, #a3a3a3);
    font: inherit;
    font-size: 0.68rem;
    font-weight: 650;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 0.28rem 0.65rem;
    cursor: pointer;
    transition:
      background 0.1s,
      color 0.1s;
  }

  .mode-btn:hover {
    color: var(--veil-text, #e5e5e5);
  }

  .mode-btn.active {
    background: var(--veil-accent-hover, rgba(115, 115, 115, 0.35));
    color: var(--veil-text, #e5e5e5);
  }

  .body-source-view {
    margin: 0;
    padding: 0.65rem 0.75rem;
    border: 1px solid var(--veil-border, #2e2e2e);
    border-radius: 6px;
    background: var(--veil-code-bg, #0a0a0a);
    color: var(--veil-text, #e5e5e5);
    font-family: 'JetBrains Mono', 'Fira Code', ui-monospace, monospace;
    font-size: 0.78rem;
    line-height: 1.55;
    white-space: pre-wrap;
    word-break: break-word;
    overflow: auto;
    max-height: min(50vh, 28rem);
    min-height: 4rem;
  }

  .body-source-view code {
    font: inherit;
    color: inherit;
  }

  .body-source-empty {
    margin: 0;
    padding: 0.75rem;
    font-size: 0.8rem;
    color: var(--veil-text-dim, #a3a3a3);
    font-style: italic;
  }

  .body-source-edit {
    border: 1px solid var(--veil-border, #2e2e2e);
    border-radius: 6px;
    padding: 0.5rem;
    background: var(--veil-code-bg, #0a0a0a);
    min-height: 4rem;
  }
</style>
