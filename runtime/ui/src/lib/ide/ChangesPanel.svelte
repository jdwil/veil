<script lang="ts">
  /**
   * Git-shaped Changes sidebar: Uncommitted working tree + Commit history.
   * No "vs baseline" structural IR mode — that confused operators with git status.
   */
  import { onMount } from 'svelte';
  import {
    sessionChanges,
    changesRevision,
    selectedChangeReview,
    clearChangeReview,
    diagnostics,
    checkMeta,
    codingSessionMeta,
    codingSessionRevision,
    getCodingSessionId,
    ideApiBase,
    ideRequestHeaders,
    type SessionChangeEntry,
    type ChangeReviewItem,
  } from '$lib/ide/store';
  import { getNodeStyle, type NodeKind } from '$lib/ide/types';
  import { openPrWizard } from '$lib/ide/prWizard';

  interface CommitRow {
    commit_id: string;
    message: string;
    created_at: string;
    parent?: string | null;
    revision?: number;
    branch_name?: string | null;
    files?: string[];
  }

  type Mode = 'uncommitted' | 'history';

  let mode = $state<Mode>('uncommitted');
  let loading = $state(false);
  let error = $state<string | null>(null);
  let commits = $state<CommitRow[]>([]);

  const selectedKey = $derived($selectedChangeReview?.key ?? null);

  const errCount = $derived(
    $checkMeta?.error_count ??
      $diagnostics.filter((d) => (d.severity ?? '').toLowerCase() === 'error').length
  );
  const warnCount = $derived(
    $checkMeta?.warning_count ??
      Math.max(0, $diagnostics.length - errCount)
  );

  const meta = $derived($codingSessionMeta as Record<string, unknown> | null);
  const branchName = $derived(
    (meta?.branch_name as string) ||
      (meta?.draft_mode ? 'work' : (meta?.branch as string) || 'main')
  );
  const baseBranch = $derived(
    (meta?.base_branch as string) || (meta?.branch as string) || 'main'
  );
  const uncommitted = $derived(!!meta?.uncommitted || $sessionChanges.length > 0);
  const isFeature = $derived(!!meta?.draft_mode);
  const rev = $derived(
    $codingSessionRevision ?? (typeof meta?.revision === 'number' ? meta.revision : null)
  );

  function apiRoot(): string {
    const base = ideApiBase();
    const u = base.replace(/\/api\/p\/[^/]+\/?$/, '');
    return u || (typeof window !== 'undefined' ? window.location.origin : '');
  }

  async function loadCommits() {
    const id = getCodingSessionId();
    if (!id) {
      commits = [];
      return;
    }
    loading = true;
    error = null;
    try {
      const r = await fetch(`${apiRoot()}/api/sessions/${id}/commits`, {
        headers: ideRequestHeaders(),
      });
      if (!r.ok) {
        error = `HTTP ${r.status}`;
        commits = [];
        return;
      }
      const data = await r.json();
      commits = data.commits || [];
    } catch (e) {
      error = String(e);
      commits = [];
    } finally {
      loading = false;
    }
  }

  async function refreshSessionMeta() {
    const id = getCodingSessionId();
    if (!id) return;
    try {
      const r = await fetch(`${apiRoot()}/api/sessions/${id}`, {
        headers: ideRequestHeaders(),
      });
      if (!r.ok) return;
      const data = await r.json();
      if (data.session) {
        const { codingSessionMeta: m, codingSessionRevision: cr } = await import('./store');
        m.set(data.session);
        if (typeof data.session.revision === 'number') cr.set(data.session.revision);
      }
    } catch {
      /* ignore */
    }
  }

  $effect(() => {
    void $changesRevision;
    void $codingSessionRevision;
    if (mode === 'history') void loadCommits();
  });

  onMount(() => {
    void refreshSessionMeta();
    void loadCommits();
  });

  function selectSession(e: SessionChangeEntry) {
    const item: ChangeReviewItem = {
      source: 'uncommitted',
      key: e.id,
      title: `${e.subkind || e.kind} ${e.name}`,
      changeKind: e.change,
      name: e.name,
      nodeId: e.nodeId,
      subkind: e.subkind,
      at: e.at,
    };
    selectedChangeReview.set(item);
  }

  function selectCommit(c: CommitRow) {
    const item: ChangeReviewItem = {
      source: 'commit',
      key: `commit:${c.commit_id}`,
      title: c.message || c.commit_id.slice(0, 8),
      changeKind: 'commit',
      name: null,
      nodeId: null,
      subkind: null,
      commitId: c.commit_id,
      commitMessage: c.message,
      commitFiles: c.files ?? [],
      baseLabel: c.parent ? c.parent.slice(0, 8) : '(root)',
      headLabel: c.commit_id.slice(0, 8),
      at: c.created_at ? Date.parse(c.created_at) : null,
    };
    selectedChangeReview.set(item);
  }

  function styleFor(kind: string, subkind: string | null | undefined) {
    return getNodeStyle(kind as NodeKind, subkind);
  }

  function sessionLabel(e: SessionChangeEntry): string {
    const sk = e.subkind || e.kind;
    return `${sk} ${e.name}`;
  }

  function timeAgo(at: number): string {
    const s = Math.max(0, Math.floor((Date.now() - at) / 1000));
    if (s < 5) return 'just now';
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    return `${Math.floor(s / 3600)}h ago`;
  }

  function changeClass(c: string): string {
    if (c === 'added' || c === 'add') return 'add';
    if (c === 'removed' || c === 'rem') return 'rem';
    return 'chg';
  }

  const sessionCount = $derived($sessionChanges.length);
  const commitCount = $derived(commits.length);
</script>

<div class="changes-panel">
  <div class="explain">
    <div class="explain-row">
      <span class="label">Branch</span>
      <span class="branch" class:feature={isFeature} title="Active work line">
        ⎇ {branchName}
        {#if uncommitted}<span class="dot" title="Uncommitted work"></span>{/if}
        {#if rev != null}<span class="rev">r{rev}</span>{/if}
      </span>
    </div>
    <div class="explain-row">
      <span class="label">Package check</span>
      <span class="counts" title="veil check scoreboard — not change list size">
        <span class="e">{errCount} err</span>
        <span class="w">{warnCount} warn</span>
      </span>
    </div>
    <p class="hint">
      Working tree · commits · review as a PR. Use the top-bar
      <strong>Commit</strong> / branch chip for checkpoints. Autosave is not a commit.
      Agents open a PR — you walk changes in the PR Wizard.
    </p>
    <button
      type="button"
      class="prw-btn"
      title="Walk each structural change with agent rationales — approve or send feedback"
      onclick={() => openPrWizard(null)}
    >
      ✦ PR Wizard
    </button>
  </div>

  <div class="changes-modes" role="tablist" aria-label="Change source">
    <button
      type="button"
      class="mode-btn"
      class:active={mode === 'uncommitted'}
      role="tab"
      aria-selected={mode === 'uncommitted'}
      onclick={() => {
        mode = 'uncommitted';
      }}
      title="Constructs edited since this page load (working tree)"
    >
      Uncommitted
      {#if sessionCount > 0}
        <span class="mode-count">{sessionCount}</span>
      {:else if uncommitted}
        <span class="mode-count dirty">·</span>
      {/if}
    </button>
    <button
      type="button"
      class="mode-btn"
      class:active={mode === 'history'}
      role="tab"
      aria-selected={mode === 'history'}
      onclick={() => {
        mode = 'history';
        void loadCommits();
      }}
      title="Named commits on this branch"
    >
      History
      {#if commitCount > 0}
        <span class="mode-count">{commitCount}</span>
      {/if}
    </button>
    <button
      type="button"
      class="refresh-btn"
      title="Refresh"
      disabled={loading}
      onclick={() => {
        void refreshSessionMeta();
        void loadCommits();
      }}
    >
      {loading ? '…' : '↻'}
    </button>
  </div>

  {#if mode === 'uncommitted'}
    <p class="banner">
      Working tree on <code>{branchName}</code>
      {#if uncommitted}
        · uncommitted changes
      {:else}
        · clean
      {/if}
    </p>
  {:else}
    <p class="banner">
      Commits on <code>{branchName}</code>
      {#if isFeature}
        · merge to {baseBranch} when ready
      {/if}
    </p>
  {/if}

  <div class="changes-list" role="list">
    {#if mode === 'uncommitted'}
      {#if $sessionChanges.length === 0}
        <p class="empty">
          {#if uncommitted}
            Session has uncommitted work (autosaved). Construct-level edits appear here when the
            agent or you change IR this load. Use top-bar <strong>Commit</strong> for a named
            checkpoint.
          {:else}
            Working tree is clean. Edits from you or the agent show up here until you commit.
          {/if}
        </p>
      {:else}
        {#each $sessionChanges as e (e.id)}
          {@const st = styleFor(e.kind, e.subkind)}
          <button
            type="button"
            class="change-row {changeClass(e.change)}"
            class:selected={selectedKey === e.id}
            role="listitem"
            onclick={() => selectSession(e)}
          >
            <span class="icon" style="color: {st.color}">{st.icon}</span>
            <span class="meta">
              <span class="name">{sessionLabel(e)}</span>
              <span class="sub">
                <span class="tag">{e.change}</span>
                · {timeAgo(e.at)}
              </span>
            </span>
          </button>
        {/each}
      {/if}
    {:else if error}
      <p class="err">{error}</p>
      <p class="empty">Could not load commit history. Is a coding session open?</p>
    {:else if loading && commits.length === 0}
      <p class="empty">Loading commits…</p>
    {:else if commits.length === 0}
      <p class="empty">
        No commits yet on this branch. After a successful fix slice, create a commit with a
        message (top bar or agent <code>session_commit</code>).
      </p>
    {:else}
      {#each commits as c (c.commit_id)}
        <button
          type="button"
          class="change-row chg"
          class:selected={selectedKey === `commit:${c.commit_id}`}
          role="listitem"
          onclick={() => selectCommit(c)}
        >
          <span class="icon">●</span>
          <span class="meta">
            <span class="name">{c.message}</span>
            <span class="sub">
              <code class="cid">{c.commit_id.slice(0, 8)}</code>
              · {c.created_at?.slice(0, 19)?.replace('T', ' ') || ''}
              {#if c.files?.length}
                · {c.files.length} file{c.files.length === 1 ? '' : 's'}
              {/if}
            </span>
          </span>
        </button>
      {/each}
    {/if}
  </div>

  {#if selectedKey}
    <button type="button" class="clear-sel" onclick={() => clearChangeReview()}>
      Clear selection
    </button>
  {/if}
</div>

<style>
  .changes-panel {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .explain {
    padding: 8px 10px 6px;
    border-bottom: 1px solid var(--veil-border, #333);
    flex-shrink: 0;
  }

  .prw-btn {
    width: 100%;
    margin-top: 8px;
    padding: 0.45rem 0.6rem;
    border-radius: 8px;
    border: 1px solid rgba(59, 130, 246, 0.45);
    background: linear-gradient(180deg, rgba(59, 130, 246, 0.22), rgba(37, 99, 235, 0.18));
    color: #93c5fd;
    font-size: 0.78rem;
    font-weight: 700;
    cursor: pointer;
    letter-spacing: 0.01em;
  }
  .prw-btn:hover {
    border-color: #3b82f6;
    color: #fff;
    background: linear-gradient(180deg, rgba(59, 130, 246, 0.35), rgba(37, 99, 235, 0.28));
  }

  .explain-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }

  .label {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--veil-text-dim, #888);
  }

  .branch {
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.7rem;
    font-weight: 600;
    color: var(--veil-text, #e5e5e5);
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .branch.feature {
    color: #bfdbfe;
  }

  .branch .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #fbbf24;
  }

  .branch .rev {
    opacity: 0.55;
    font-weight: 500;
  }

  .counts {
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.7rem;
    display: inline-flex;
    gap: 8px;
  }

  .e {
    color: #f87171;
  }
  .w {
    color: #fbbf24;
  }

  .hint {
    margin: 0;
    font-size: 0.68rem;
    line-height: 1.35;
    color: var(--veil-text-dim, #888);
  }

  .changes-modes {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--veil-border, #333);
    flex-shrink: 0;
  }

  .mode-btn {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--veil-text-dim, #a3a3a3);
    font-size: 0.72rem;
    font-weight: 600;
    padding: 4px 8px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .mode-btn.active {
    border-color: var(--veil-border, #444);
    background: var(--veil-surface-alt, #1a1a1a);
    color: var(--veil-text, #e5e5e5);
  }

  .mode-count {
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.65rem;
    background: var(--veil-surface, #111);
    border-radius: 999px;
    padding: 1px 6px;
    color: var(--veil-accent, #a3a3a3);
  }

  .mode-count.dirty {
    color: #fbbf24;
  }

  .refresh-btn {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--veil-text-dim, #888);
    cursor: pointer;
    font-size: 0.85rem;
    padding: 2px 6px;
  }

  .banner {
    margin: 0;
    padding: 6px 10px;
    font-size: 0.65rem;
    line-height: 1.35;
    color: var(--veil-text-dim, #999);
    border-bottom: 1px solid var(--veil-border, #333);
    flex-shrink: 0;
  }

  .banner code {
    font-size: 0.62rem;
  }

  .changes-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 0;
  }

  .empty,
  .err {
    margin: 12px 10px;
    font-size: 0.75rem;
    color: var(--veil-text-dim, #888);
    line-height: 1.4;
  }

  .err {
    color: #f87171;
  }

  .change-row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    border-left: 2px solid transparent;
    color: var(--veil-text, #e5e5e5);
    padding: 7px 10px;
    cursor: pointer;
    font: inherit;
  }

  .change-row:hover {
    background: var(--veil-surface-alt, rgba(255, 255, 255, 0.04));
  }

  .change-row.selected {
    background: rgba(147, 197, 253, 0.1);
    border-left-color: #93c5fd;
  }

  .change-row.add {
    border-left-color: #4ade80;
  }
  .change-row.rem {
    border-left-color: #f87171;
  }
  .change-row.chg:not(.selected) {
    border-left-color: #fbbf24;
  }

  .icon {
    flex-shrink: 0;
    font-size: 0.9rem;
    line-height: 1.2;
    width: 1.2rem;
    text-align: center;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .name {
    font-size: 0.78rem;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sub {
    font-size: 0.65rem;
    color: var(--veil-text-dim, #888);
  }

  .tag {
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-weight: 600;
  }

  .cid {
    color: #93c5fd;
    font-size: 0.65rem;
  }

  .clear-sel {
    flex-shrink: 0;
    border: none;
    border-top: 1px solid var(--veil-border, #333);
    background: transparent;
    color: var(--veil-text-dim, #888);
    font-size: 0.7rem;
    padding: 8px;
    cursor: pointer;
  }

  .clear-sel:hover {
    color: var(--veil-text, #ccc);
  }
</style>
