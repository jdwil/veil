<script lang="ts">
  /**
   * Domain Layer Explorer — navigate constructs with recent design-journal entries.
   * Complements /review: same rationales/decisions, searchable after accept.
   */
  import { onMount } from 'svelte';
  import { focusConstructByName, currentProjectParam } from '$lib/ide/store';
  import { fetchJournal } from '$lib/review/pr-api';

  let q = $state('');
  let construct = $state('');
  let entries = $state<Record<string, unknown>[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      const list = await fetchJournal({
        construct: construct.trim() || undefined,
        q: q.trim() || undefined,
        limit: 40,
      });
      entries = list as Record<string, unknown>[];
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  function explainSelected() {
    const name = construct.trim();
    if (!name) return;
    const related = entries
      .filter((e) => String(e.construct_name || '').includes(name) || String(e.construct_path || '').includes(name))
      .slice(0, 5);
    const lines = [
      `Explain construct \`${name}\` in project \`${currentProjectParam() || '?'}\`.`,
      'Ground the answer in domain layers and these design-journal decisions:',
      ...related.map(
        (e, i) =>
          `${i + 1}. ${e.decision} — ${e.construct_name}: ${e.rationale || e.teaching_note || '(no note)'}`
      ),
    ];
    // Lazy import to avoid circular agent deps at module load
    import('$lib/agent/runtimeAgentSession').then(({ agentSend, openAgentPanel }) => {
      openAgentPanel();
      void agentSend(lines.join('\n'));
    });
  }
</script>

<div class="dle">
  <header>
    <h3>Domain journal</h3>
    <p class="muted">Accepted review decisions + teaching notes for this runtime.</p>
  </header>
  <div class="filters">
    <input type="search" placeholder="Construct name…" bind:value={construct} />
    <input type="search" placeholder="Search text…" bind:value={q} />
    <button type="button" class="btn" disabled={loading} onclick={() => void load()}>
      {loading ? '…' : 'Search'}
    </button>
    <button type="button" class="ghost" disabled={!construct.trim()} onclick={() => explainSelected()}>
      Explain this
    </button>
  </div>
  {#if error}
    <p class="err">{error}</p>
  {/if}
  {#if !loading && entries.length === 0}
    <p class="muted">No journal entries yet. Approve a review to grow this record.</p>
  {:else}
    <ul class="entries">
      {#each entries as e}
        <li>
          <div class="eh">
            <button
              type="button"
              class="linkish"
              onclick={() => focusConstructByName(String(e.construct_name || ''))}
            >
              {e.construct_name || '—'}
            </button>
            <span class="pill">{e.decision}</span>
            {#if e.risk}<span class="dim">{e.risk}</span>{/if}
            <span class="dim">{String(e.ts || '').slice(0, 19)}</span>
          </div>
          {#if e.rationale}
            <p class="body">{e.rationale}</p>
          {/if}
          {#if e.teaching_note}
            <p class="note"><strong>Note:</strong> {e.teaching_note}</p>
          {/if}
          <code class="path">{e.construct_path}</code>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .dle {
    padding: 0.75rem 1rem;
    height: 100%;
    overflow: auto;
    font-size: 0.85rem;
  }
  header h3 {
    margin: 0 0 0.25rem;
    font-size: 0.95rem;
  }
  .muted {
    color: #a3a3a3;
    font-size: 0.78rem;
    margin: 0 0 0.75rem;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-bottom: 0.75rem;
  }
  input {
    flex: 1;
    min-width: 8rem;
    background: rgba(0, 0, 0, 0.35);
    border: 1px solid rgba(115, 115, 115, 0.4);
    border-radius: 6px;
    color: #e5e5e5;
    padding: 0.35rem 0.5rem;
    font-size: 0.8rem;
  }
  .btn,
  .ghost {
    border-radius: 6px;
    font-size: 0.78rem;
    font-weight: 600;
    padding: 0.35rem 0.65rem;
    cursor: pointer;
    border: 1px solid rgba(115, 115, 115, 0.4);
    background: rgba(255, 255, 255, 0.08);
    color: #e5e5e5;
  }
  .ghost {
    background: transparent;
  }
  .entries {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .entries li {
    padding: 0.55rem 0.65rem;
    border: 1px solid rgba(115, 115, 115, 0.25);
    border-radius: 8px;
    margin-bottom: 0.45rem;
    background: rgba(0, 0, 0, 0.2);
  }
  .eh {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .linkish {
    background: none;
    border: none;
    color: #93c5fd;
    cursor: pointer;
    font-weight: 650;
    padding: 0;
    font-size: 0.85rem;
  }
  .pill {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    padding: 0.1rem 0.35rem;
    border-radius: 999px;
    border: 1px solid rgba(115, 115, 115, 0.4);
    color: #a3a3a3;
  }
  .dim {
    font-size: 0.72rem;
    color: #737373;
  }
  .body {
    margin: 0.35rem 0 0;
    font-size: 0.8rem;
  }
  .note {
    margin: 0.25rem 0 0;
    font-size: 0.78rem;
    color: #c4b5fd;
  }
  .path {
    display: block;
    margin-top: 0.35rem;
    font-size: 0.7rem;
    color: #737373;
  }
  .err {
    color: #fca5a5;
  }
</style>
