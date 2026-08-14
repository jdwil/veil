<script lang="ts">
  /**
   * Main-pane review when the Changes tab has a selection.
   * Shows what actually changed (before/after) + related diagnostics —
   * not the same as Outline construct editor.
   */
  import {
    selectedChangeReview,
    clearChangeReview,
    diagnostics,
    selectedNodeId,
    focusDiagnostic,
    type ChangeReviewItem,
    type Diagnostic,
  } from '$lib/ide/store';
  import { getNodeStyle, paletteStylesVersion, type NodeKind } from '$lib/ide/types';

  let review = $derived($selectedChangeReview);

  let relatedDiags = $derived.by((): Diagnostic[] => {
    const r = review;
    if (!r) return [];
    return $diagnostics.filter((d) => {
      if (r.nodeId != null && d.node_id != null && d.node_id === r.nodeId) return true;
      if (r.name && d.node_name && d.node_name === r.name) return true;
      if (r.name && d.message?.includes(r.name)) return true;
      return false;
    });
  });

  let errN = $derived(
    relatedDiags.filter((d) => (d.severity ?? '').toLowerCase() === 'error').length
  );
  let warnN = $derived(relatedDiags.length - errN);

  let reviewStyle = $derived.by(() => {
    void $paletteStylesVersion;
    const r = review;
    if (!r) return null;
    return getNodeStyle('TypeDef' as NodeKind, r.subkind);
  });

  function openConstruct(r: ChangeReviewItem) {
    if (r.nodeId != null) {
      selectedNodeId.set(String(r.nodeId));
    } else if (r.name) {
      focusDiagnostic({
        severity: 'Warning',
        message: 'jump to construct',
        node_name: r.name,
      });
    }
  }

  function goDiag(d: Diagnostic) {
    focusDiagnostic(d);
  }

  function kindLabel(k: string): string {
    return k.replaceAll('_', ' ');
  }
</script>

{#if !review}
  <div class="empty-review">
    <h2>Review changes</h2>
    <p>
      Select an item in the <strong>Changes</strong> list (Uncommitted working tree or
      History commits). This pane is for review — not the Outline property editor.
    </p>
    <ul class="legend">
      <li>
        <strong>Uncommitted</strong> — constructs edited in this session (working tree).
        Commit when a slice is done.
      </li>
      <li>
        <strong>History</strong> — named commits on the current branch (message + files).
      </li>
      <li>
        <strong>Errors / warnings</strong> (top bar) — live <code>veil check</code> scoreboard.
        Not the same as the change list size.
      </li>
      <li>
        <strong>PR Wizard</strong> — top-bar <em>Review</em> or Changes → <em>PR Wizard</em>.
        Walk each structural change with agent rationale; approve or send feedback.
        Agents open PRs; humans merge after review.
      </li>
    </ul>
  </div>
{:else}
  {@const st = reviewStyle}
  <div class="review">
    <header class="review-head">
      <div class="title-row">
        <span class="icon" style="color: {st?.color}">{st?.icon}</span>
        <div class="titles">
          <h2>{review.title}</h2>
          <p class="sub">
            <span class="pill source">
              {#if review.source === 'commit'}
                Commit
              {:else if review.source === 'uncommitted' || review.source === 'session'}
                Uncommitted
              {:else}
                Change
              {/if}
            </span>
            <span class="pill kind">{kindLabel(review.changeKind)}</span>
            {#if review.subkind}
              <span class="pill">{review.subkind}</span>
            {/if}
            {#if review.commitId}
              <span class="pill dim"><code>{review.commitId.slice(0, 8)}</code></span>
            {/if}
            {#if review.baseLabel && review.headLabel && review.source === 'commit'}
              <span class="pill dim">{review.baseLabel} → {review.headLabel}</span>
            {/if}
          </p>
        </div>
        <button type="button" class="close" onclick={() => clearChangeReview()} title="Clear selection">
          ✕
        </button>
      </div>
      <div class="actions">
        {#if review.name || review.nodeId != null}
          <button type="button" class="btn" onclick={() => openConstruct(review)}>
            Open construct in editor
          </button>
        {/if}
      </div>
    </header>

    <section class="section">
      <h3>What changed</h3>
      {#if review.source === 'commit'}
        <p class="note">
          <strong>Commit</strong>
          {#if review.commitId}
            <code>{review.commitId.slice(0, 8)}</code>
          {/if}
          {#if review.commitMessage}
            — {review.commitMessage}
          {/if}
        </p>
        {#if review.commitFiles && review.commitFiles.length > 0}
          <ul class="file-list">
            {#each review.commitFiles as f}
              <li><code>{f}</code></li>
            {/each}
          </ul>
        {:else}
          <p class="muted">Snapshot recorded at commit time (file list may be empty for older commits).</p>
        {/if}
      {:else if review.beforePreview?.length || review.afterPreview?.length || review.beforeText || review.afterText}
        <div class="diff-grid">
          <div class="col before">
            <div class="col-h">Before</div>
            {#if review.beforeText}
              <pre class="block rem">{review.beforeText}</pre>
            {:else if review.beforePreview?.length}
              {#each review.beforePreview as line}
                <div class="line rem">− {line}</div>
              {/each}
            {:else}
              <p class="muted">No before snapshot</p>
            {/if}
          </div>
          <div class="col after">
            <div class="col-h">After</div>
            {#if review.afterText}
              <pre class="block add">{review.afterText}</pre>
            {:else if review.afterPreview?.length}
              {#each review.afterPreview as line}
                <div class="line add">+ {line}</div>
              {/each}
            {:else}
              <p class="muted">No after snapshot</p>
            {/if}
          </div>
        </div>
      {:else if review.changeKind === 'added'}
        <p class="note">
          <strong>Added</strong> construct <code>{review.name}</code> in the working tree.
          No line-level body preview for this item.
        </p>
      {:else if review.changeKind === 'removed'}
        <p class="note">
          <strong>Removed</strong> construct <code>{review.name}</code>. It is gone from the current IR.
        </p>
      {:else}
        <p class="note">
          Marked <strong>{kindLabel(review.changeKind)}</strong>
          {#if review.source === 'session'}
            in this session (IR fingerprint changed). Open the construct to inspect current fields/body.
            For line-level body diffs, use <strong>vs baseline</strong> when a git base exists.
          {:else}
            in the structural diff. Preview text was not attached for this item type.
          {/if}
        </p>
      {/if}
    </section>

    <section class="section">
      <h3>
        Related diagnostics
        {#if relatedDiags.length > 0}
          <span class="diag-counts">
            {#if errN > 0}<span class="e">{errN} err</span>{/if}
            {#if warnN > 0}<span class="w">{warnN} warn</span>{/if}
          </span>
        {/if}
      </h3>
      {#if relatedDiags.length === 0}
        <p class="muted">
          No current check diagnostics name this construct. Package-wide counts stay in the top bar
          (error/warning scoreboard — not the same as the change list).
        </p>
      {:else}
        <ul class="diag-list">
          {#each relatedDiags as d}
            <li>
              <button
                type="button"
                class="diag-btn"
                class:error={(d.severity ?? '').toLowerCase() === 'error'}
                onclick={() => goDiag(d)}
              >
                <span class="sev"
                  >{(d.severity ?? '').toLowerCase() === 'error' ? '⛔' : '⚠️'}</span
                >
                <span class="msg">
                  {#if d.code}<code>[{d.code}]</code>{/if}
                  {d.message}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
{/if}

<style>
  .empty-review,
  .review {
    height: 100%;
    overflow-y: auto;
    padding: 20px 24px 32px;
    color: var(--veil-text, #e5e5e5);
  }

  .empty-review h2,
  .review h2 {
    margin: 0 0 8px;
    font-size: 1.1rem;
    font-weight: 650;
  }

  .empty-review p,
  .note,
  .muted {
    font-size: 0.85rem;
    line-height: 1.5;
    color: var(--veil-text-dim, #a3a3a3);
    margin: 0 0 12px;
  }

  .legend {
    margin: 16px 0 0;
    padding-left: 1.2rem;
    font-size: 0.8rem;
    line-height: 1.55;
    color: var(--veil-text-dim, #a3a3a3);
  }

  .legend li {
    margin-bottom: 8px;
  }

  .legend code {
    font-size: 0.75rem;
  }

  .file-list {
    margin: 0 0 12px;
    padding-left: 1.2rem;
    font-size: 0.8rem;
    line-height: 1.5;
  }

  .file-list code {
    font-size: 0.75rem;
  }

  .review-head {
    margin-bottom: 20px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--veil-border, #333);
  }

  .title-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .icon {
    font-size: 1.4rem;
    line-height: 1.2;
  }

  .titles {
    flex: 1;
    min-width: 0;
  }

  .sub {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 6px 0 0;
  }

  .pill {
    font-size: 0.65rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--veil-surface-alt, #1a1a1a);
    border: 1px solid var(--veil-border, #333);
    color: var(--veil-text-dim, #ccc);
  }

  .pill.source {
    color: #93c5fd;
    border-color: rgba(147, 197, 253, 0.35);
  }

  .pill.dim {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 500;
    opacity: 0.85;
  }

  .close {
    background: none;
    border: none;
    color: var(--veil-text-dim, #888);
    cursor: pointer;
    font-size: 1rem;
    padding: 4px 8px;
  }

  .actions {
    margin-top: 12px;
  }

  .btn {
    background: var(--veil-surface-alt, #1a1a1a);
    border: 1px solid var(--veil-border, #444);
    border-radius: 6px;
    color: var(--veil-text, #e5e5e5);
    font-size: 0.75rem;
    padding: 6px 12px;
    cursor: pointer;
  }

  .btn:hover {
    border-color: var(--veil-text-dim, #888);
  }

  .section {
    margin-bottom: 22px;
  }

  .section h3 {
    margin: 0 0 10px;
    font-size: 0.75rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--veil-text-dim, #a3a3a3);
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .diag-counts {
    display: inline-flex;
    gap: 8px;
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.7rem;
    text-transform: none;
    letter-spacing: 0;
  }

  .e {
    color: #f87171;
  }
  .w {
    color: #fbbf24;
  }

  .diff-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    min-height: 120px;
  }

  @media (max-width: 900px) {
    .diff-grid {
      grid-template-columns: 1fr;
    }
  }

  .col {
    border: 1px solid var(--veil-border, #333);
    border-radius: 8px;
    background: var(--veil-surface-alt, #121212);
    overflow: hidden;
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.72rem;
    line-height: 1.4;
  }

  .col-h {
    padding: 6px 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-size: 0.65rem;
    border-bottom: 1px solid var(--veil-border, #333);
    color: var(--veil-text-dim, #888);
  }

  .line,
  .block {
    padding: 4px 10px;
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .block {
    padding: 10px;
  }

  .line.rem,
  .block.rem {
    color: #fca5a5;
    background: rgba(248, 113, 113, 0.06);
  }

  .line.add,
  .block.add {
    color: #86efac;
    background: rgba(74, 222, 128, 0.06);
  }

  .diag-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .diag-btn {
    display: flex;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: inherit;
    padding: 8px 10px;
    cursor: pointer;
    font: inherit;
    font-size: 0.8rem;
    margin-bottom: 4px;
  }

  .diag-btn:hover {
    background: var(--veil-surface-alt, #1a1a1a);
    border-color: var(--veil-border, #333);
  }

  .diag-btn.error .msg {
    color: #fecaca;
  }

  .msg code {
    font-size: 0.72rem;
    margin-right: 4px;
    color: var(--veil-text-dim, #aaa);
  }

  .note code {
    font-size: 0.8rem;
  }
</style>
