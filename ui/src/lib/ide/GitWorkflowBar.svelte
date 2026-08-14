<script lang="ts">
  /**
   * Git-shaped session workflow: branch · uncommitted · commit · log · merge.
   */
  import {
    codingSessionMeta,
    codingSessionRevision,
    getCodingSessionId,
    setCodingSessionId,
    ensureCodingSession,
    currentProjectParam,
    ideApiBase,
    ideRequestHeaders,
    sessionSaveState,
  } from './store';
  import { openPrWizard } from './prWizard';

  interface CommitRow {
    commit_id: string;
    message: string;
    created_at: string;
    parent?: string | null;
    revision?: number;
    branch_name?: string | null;
  }

  let open = $state(false);
  let commitOpen = $state(false);
  let branchOpen = $state(false);
  let message = $state('');
  let newBranch = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);
  let commits = $state<CommitRow[]>([]);
  let logOpen = $state(false);

  const meta = $derived($codingSessionMeta as Record<string, unknown> | null);
  const branchName = $derived(
    (meta?.branch_name as string) ||
      (meta?.draft_mode ? 'work' : (meta?.branch as string) || 'main')
  );
  const baseBranch = $derived((meta?.base_branch as string) || (meta?.branch as string) || 'main');
  const uncommitted = $derived(!!meta?.uncommitted);
  const isFeature = $derived(!!meta?.draft_mode);
  const headCommit = $derived((meta?.head_commit as string) || null);
  const rev = $derived($codingSessionRevision);

  async function refreshMeta() {
    const id = getCodingSessionId();
    if (!id) return;
    try {
      const r = await fetch(`${ideApiBase().replace(/\/api\/p\/[^/]+$/, '')}/api/sessions/${id}`, {
        headers: ideRequestHeaders(),
      });
      // sessions API is on host root, not /api/p/{slug}
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

  function apiRoot(): string {
    // ideApiBase is like http://host/api/p/relay — sessions live on /api/sessions
    const base = ideApiBase();
    const u = base.replace(/\/api\/p\/[^/]+\/?$/, '');
    return u || (typeof window !== 'undefined' ? window.location.origin : '');
  }

  async function loadCommits() {
    const id = getCodingSessionId();
    if (!id) return;
    try {
      const r = await fetch(`${apiRoot()}/api/sessions/${id}/commits`, {
        headers: ideRequestHeaders(),
      });
      if (!r.ok) return;
      const data = await r.json();
      commits = data.commits || [];
    } catch {
      commits = [];
    }
  }

  async function doCommit() {
    const id = getCodingSessionId();
    if (!id || !message.trim()) return;
    busy = true;
    err = null;
    try {
      const r = await fetch(`${apiRoot()}/api/sessions/${id}/commits`, {
        method: 'POST',
        headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: message.trim() }),
      });
      const data = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
      message = '';
      commitOpen = false;
      if (data.session) {
        const { codingSessionMeta: m, codingSessionRevision: cr } = await import('./store');
        m.set(data.session);
        if (typeof data.session.revision === 'number') cr.set(data.session.revision);
      }
      await loadCommits();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  async function createBranch() {
    const slug = currentProjectParam();
    if (!slug || !newBranch.trim()) return;
    busy = true;
    err = null;
    try {
      const r = await fetch(`${apiRoot()}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          slug,
          branch_name: newBranch.trim(),
        }),
      });
      const data = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
      const sid = data.session?.session_id;
      if (sid) {
        setCodingSessionId(sid);
        const { codingSessionMeta: m, codingSessionRevision: cr } = await import('./store');
        m.set(data.session);
        if (typeof data.session.revision === 'number') cr.set(data.session.revision);
      }
      newBranch = '';
      branchOpen = false;
      // Reload IDE into new worktree
      window.location.reload();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  async function switchMain() {
    const slug = currentProjectParam();
    if (!slug) return;
    busy = true;
    try {
      // Sticky mainline session
      const r = await fetch(`${apiRoot()}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ slug }),
      });
      const data = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
      const sid = data.session?.session_id;
      if (sid) setCodingSessionId(sid);
      window.location.reload();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    if (open || logOpen) void loadCommits();
  });
</script>

<div class="git-bar">
  <button
    type="button"
    class="branch-chip"
    class:feature={isFeature}
    class:dirty={uncommitted}
    title="Git-shaped work line (not raw git). Click for branch actions."
    onclick={() => {
      open = !open;
      branchOpen = false;
      commitOpen = false;
    }}
  >
    <span class="icon">⎇</span>
    <span class="name">{branchName}</span>
    {#if uncommitted}
      <span class="dot" title="Uncommitted changes"></span>
    {/if}
    {#if rev != null}
      <span class="rev">r{rev}</span>
    {/if}
  </button>

  <button
    type="button"
    class="action"
    class:primary={uncommitted}
    disabled={busy || !getCodingSessionId()}
    title="Create a named commit (checkpoint)"
    onclick={() => {
      commitOpen = !commitOpen;
      open = false;
      branchOpen = false;
    }}
  >
    Commit
  </button>

  <button
    type="button"
    class="action review"
    title="PR Wizard — walk structural changes, approve or send feedback to the agent"
    onclick={() => openPrWizard(null)}
  >
    Review
  </button>
  <a
    class="action"
    href={`/review/${encodeURIComponent(currentProjectParam() || '')}`}
    title="Outstanding change set — human sign-off (not git status)"
  >
    Sign off
  </a>

  <!-- Session merge to main is disabled — use PR Wizard → Approve → Merge -->

  {#if commitOpen}
    <div class="popover commit-pop">
      <label>
        Commit message
        <input
          type="text"
          placeholder="fix: describe what improved"
          bind:value={message}
          onkeydown={(e) => {
            if (e.key === 'Enter') void doCommit();
            if (e.key === 'Escape') commitOpen = false;
          }}
        />
      </label>
      <div class="row">
        <button type="button" class="action primary" disabled={busy || !message.trim()} onclick={() => void doCommit()}>
          {busy ? '…' : 'Create commit'}
        </button>
        <button type="button" class="action" onclick={() => (commitOpen = false)}>Cancel</button>
      </div>
      {#if err}<p class="err">{err}</p>{/if}
      <p class="hint">
        Saves a named snapshot of this branch. Autosave already keeps work durable — commits are history.
      </p>
    </div>
  {/if}

  {#if open}
    <div class="popover menu-pop">
      <div class="menu-head">
        <strong>{branchName}</strong>
        <span class="sub">base {baseBranch}{headCommit ? ` · head ${headCommit.slice(0, 8)}` : ''}</span>
      </div>
      <button
        type="button"
        class="menu-item"
        onclick={() => {
          branchOpen = true;
          open = false;
        }}
      >
        New branch…
      </button>
      <button
        type="button"
        class="menu-item"
        onclick={() => {
          logOpen = !logOpen;
          void loadCommits();
        }}
      >
        {logOpen ? 'Hide' : 'Show'} commit log
      </button>
      {#if isFeature}
        <button type="button" class="menu-item" onclick={() => void switchMain()}>
          Switch to {baseBranch} (mainline session)
        </button>
        <button
          type="button"
          class="menu-item"
          onclick={() => {
            open = false;
            openPrWizard(null);
          }}
        >
          Review / land via PR Wizard…
        </button>
      {/if}
      <button
        type="button"
        class="menu-item"
        onclick={async () => {
          const p = currentProjectParam();
          if (p) await ensureCodingSession(p);
          await refreshMeta();
        }}
      >
        Refresh status
      </button>
      {#if logOpen}
        <div class="log">
          {#if commits.length === 0}
            <p class="empty">No commits yet on this branch.</p>
          {:else}
            {#each commits as c}
              <div class="log-item">
                <code class="cid">{c.commit_id.slice(0, 8)}</code>
                <span class="msg">{c.message}</span>
                <span class="when">{c.created_at?.slice(0, 19)?.replace('T', ' ')}</span>
              </div>
            {/each}
          {/if}
        </div>
      {/if}
      {#if err}<p class="err">{err}</p>{/if}
      <p class="hint">
        Branch ≈ isolated session. Commit ≈ named snapshot. Land via <strong>PR Wizard</strong> (Review), not session merge.
        Save status: {$sessionSaveState}.
      </p>
    </div>
  {/if}

  {#if branchOpen}
    <div class="popover commit-pop">
      <label>
        New branch name
        <input
          type="text"
          placeholder="fix-relay-diagnostics"
          bind:value={newBranch}
          onkeydown={(e) => {
            if (e.key === 'Enter') void createBranch();
            if (e.key === 'Escape') branchOpen = false;
          }}
        />
      </label>
      <div class="row">
        <button
          type="button"
          class="action primary"
          disabled={busy || !newBranch.trim()}
          onclick={() => void createBranch()}
        >
          {busy ? '…' : 'Create branch'}
        </button>
        <button type="button" class="action" onclick={() => (branchOpen = false)}>Cancel</button>
      </div>
      <p class="hint">Starts from {baseBranch}, isolates writes (draft prefix), reloads IDE on that branch.</p>
      {#if err}<p class="err">{err}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .git-bar {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .branch-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--veil-surface-alt, #1a1a1a);
    border: 1px solid var(--veil-border, #333);
    border-radius: 8px;
    color: var(--veil-text, #e5e5e5);
    font-size: 0.72rem;
    font-weight: 600;
    padding: 5px 10px;
    cursor: pointer;
    font-family: 'JetBrains Mono', ui-monospace, monospace;
  }

  .branch-chip.feature {
    border-color: rgba(147, 197, 253, 0.45);
    color: #bfdbfe;
  }

  .branch-chip.dirty .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #fbbf24;
  }

  .icon {
    opacity: 0.8;
  }

  .rev {
    opacity: 0.55;
    font-weight: 500;
  }

  .action {
    background: transparent;
    border: 1px solid var(--veil-border, #333);
    border-radius: 6px;
    color: var(--veil-text-dim, #a3a3a3);
    font-size: 0.7rem;
    font-weight: 600;
    padding: 4px 8px;
    cursor: pointer;
    text-decoration: none;
    display: inline-flex;
    align-items: center;
  }

  .action.primary {
    border-color: rgba(74, 222, 128, 0.45);
    color: #86efac;
  }

  .action.review {
    border-color: rgba(59, 130, 246, 0.55);
    color: #93c5fd;
  }
  .action.review:hover:not(:disabled) {
    background: rgba(59, 130, 246, 0.18);
    color: #fff;
  }
  .action.merge {
    border-color: rgba(147, 197, 253, 0.4);
    color: #93c5fd;
  }

  .action:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .popover {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 80;
    min-width: 280px;
    max-width: 360px;
    background: var(--veil-surface, #0f0f0f);
    border: 1px solid var(--veil-border, #333);
    border-radius: 10px;
    padding: 12px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
  }

  .menu-pop {
    min-width: 300px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 0.7rem;
    font-weight: 600;
    color: var(--veil-text-dim, #aaa);
  }

  input {
    background: var(--veil-surface-alt, #1a1a1a);
    border: 1px solid var(--veil-border, #333);
    border-radius: 6px;
    color: var(--veil-text, #eee);
    padding: 8px 10px;
    font-size: 0.8rem;
  }

  .row {
    display: flex;
    gap: 8px;
    margin-top: 10px;
  }

  .hint,
  .empty,
  .err {
    margin: 10px 0 0;
    font-size: 0.68rem;
    line-height: 1.4;
    color: var(--veil-text-dim, #888);
  }

  .err {
    color: #f87171;
  }

  .menu-head {
    margin-bottom: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--veil-border, #333);
  }

  .menu-head .sub {
    display: block;
    font-size: 0.65rem;
    color: var(--veil-text-dim, #888);
    margin-top: 2px;
  }

  .menu-item {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--veil-text, #e5e5e5);
    font-size: 0.78rem;
    padding: 8px 6px;
    border-radius: 6px;
    cursor: pointer;
  }

  .menu-item:hover {
    background: var(--veil-surface-alt, #1a1a1a);
  }

  .log {
    margin-top: 8px;
    max-height: 180px;
    overflow-y: auto;
    border-top: 1px solid var(--veil-border, #333);
    padding-top: 8px;
  }

  .log-item {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 8px;
    font-size: 0.7rem;
    padding: 6px 0;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
  }

  .cid {
    color: #93c5fd;
    font-size: 0.65rem;
  }

  .msg {
    grid-column: 2;
    color: var(--veil-text, #ddd);
  }

  .when {
    grid-column: 2;
    color: var(--veil-text-dim, #666);
    font-size: 0.62rem;
  }
</style>
