<script lang="ts">
  /**
   * UX-021: structural / semantic diff panel (vs git HEAD).
   */
  import { focusDiagnostic } from '$lib/store';

  interface DiffItem {
    kind: string;
    path?: string;
    node_kind?: string;
    name?: string;
    from_name?: string;
    to_name?: string;
    subkind?: string | null;
    before?: string | string[];
    after?: string | string[];
    before_preview?: string[];
    after_preview?: string[];
    before_lines?: number;
    after_lines?: number;
  }

  interface StructDiff {
    base_label: string;
    head_label: string;
    items: DiffItem[];
    added: number;
    removed: number;
    changed: number;
  }

  let open = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let diff = $state<StructDiff | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      const res = await fetch('http://localhost:3001/api/diff');
      if (!res.ok) {
        error = `HTTP ${res.status}: ${await res.text()}`;
        diff = null;
        return;
      }
      diff = await res.json();
    } catch (e) {
      error = String(e);
      diff = null;
    } finally {
      loading = false;
    }
  }

  async function toggle() {
    open = !open;
    if (open) await load();
  }

  function itemLabel(i: DiffItem): string {
    const sk = i.subkind ? ` (${i.subkind})` : '';
    const path = i.path ? `${i.path}/` : '';
    switch (i.kind) {
      case 'added':
        return `+ ${path}${i.name}${sk}`;
      case 'removed':
        return `− ${path}${i.name}${sk}`;
      case 'renamed':
        return `~ ${path}${i.from_name} → ${i.to_name}${sk}`;
      case 'signature_changed':
        return `sig ${path}${i.name}`;
      case 'body_changed':
        return `body ${path}${i.name} (${i.before_lines}→${i.after_lines})`;
      case 'annotations_changed':
        return `@ ${path}${i.name}`;
      default:
        return i.kind;
    }
  }

  function jump(i: DiffItem) {
    const name = i.name ?? i.to_name;
    if (!name) return;
    focusDiagnostic({
      severity: 'Warning',
      message: 'diff jump',
      node_name: name,
    });
  }

  function kindClass(k: string): string {
    if (k === 'added') return 'add';
    if (k === 'removed') return 'rem';
    return 'chg';
  }
</script>

<div class="diff-wrap">
  <button type="button" class="diff-toggle" onclick={toggle} title="Structural diff vs git HEAD">
    {open ? '▾' : '▸'} Review changes
    {#if diff}
      <span class="counts">
        <span class="c-add">+{diff.added}</span>
        <span class="c-rem">−{diff.removed}</span>
        <span class="c-chg">~{diff.changed}</span>
      </span>
    {/if}
  </button>

  {#if open}
    <div class="diff-panel">
      <div class="diff-head">
        <span class="meta">
          {#if diff}
            {diff.base_label} → {diff.head_label}
          {:else}
            Structural diff
          {/if}
        </span>
        <button type="button" class="refresh" onclick={load} disabled={loading}>
          {loading ? '…' : 'Refresh'}
        </button>
      </div>
      {#if error}
        <p class="err">{error}</p>
      {:else if loading && !diff}
        <p class="empty">Loading…</p>
      {:else if diff && diff.items.length === 0}
        <p class="empty">No structural changes vs {diff.base_label}.</p>
      {:else if diff}
        <ul class="items">
          {#each diff.items as i}
            <li class={kindClass(i.kind)}>
              <button type="button" class="item-btn" onclick={() => jump(i)}>
                <span class="kind-tag">{i.kind.replace('_', ' ')}</span>
                <span class="label">{itemLabel(i)}</span>
              </button>
              {#if i.kind === 'body_changed'}
                <div class="body-cols">
                  <div class="col before">
                    {#each i.before_preview ?? [] as line}
                      <div class="line">− {line}</div>
                    {/each}
                  </div>
                  <div class="col after">
                    {#each i.after_preview ?? [] as line}
                      <div class="line">+ {line}</div>
                    {/each}
                  </div>
                </div>
              {:else if i.kind === 'signature_changed'}
                <div class="sig">
                  <div class="line rem-t">{i.before}</div>
                  <div class="line add-t">{i.after}</div>
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .diff-wrap {
    position: relative;
  }
  .diff-toggle {
    background: none;
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    color: var(--veil-text-dim);
    font-size: 11px;
    padding: 4px 8px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .counts {
    display: inline-flex;
    gap: 6px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
  }
  .c-add {
    color: #4ade80;
  }
  .c-rem {
    color: #f87171;
  }
  .c-chg {
    color: #fbbf24;
  }
  .diff-panel {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    width: min(420px, 90vw);
    max-height: 420px;
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    z-index: 50;
    display: flex;
    flex-direction: column;
  }
  .diff-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--veil-border);
  }
  .meta {
    font-size: 10px;
    color: var(--veil-text-faint);
  }
  .refresh {
    font-size: 10px;
    background: none;
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    color: var(--veil-text-dim);
    cursor: pointer;
    padding: 2px 6px;
  }
  .items {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
  }
  .item-btn {
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--veil-text);
    padding: 8px 12px;
    cursor: pointer;
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .item-btn:hover {
    background: var(--veil-accent-subtle);
  }
  .kind-tag {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--veil-text-faint);
    min-width: 72px;
  }
  .label {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
  }
  li.add .kind-tag {
    color: #4ade80;
  }
  li.rem .kind-tag {
    color: #f87171;
  }
  li.chg .kind-tag {
    color: #fbbf24;
  }
  .body-cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px;
    padding: 0 12px 8px;
    font-size: 10px;
    font-family: 'JetBrains Mono', monospace;
  }
  .line {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .before .line,
  .rem-t {
    color: #f87171;
  }
  .after .line,
  .add-t {
    color: #4ade80;
  }
  .sig {
    padding: 0 12px 8px;
    font-size: 10px;
    font-family: 'JetBrains Mono', monospace;
  }
  .empty,
  .err {
    padding: 12px;
    font-size: 11px;
    color: var(--veil-text-faint);
    margin: 0;
  }
  .err {
    color: #f87171;
  }
</style>
