<script lang="ts">
  /**
   * Compact durable-session status chip (saved / saving / conflict / session id).
   */
  import {
    sessionSaveState,
    sessionSaveDetail,
    codingSessionMeta,
    codingSessionRevision,
    getCodingSessionId,
    ensureCodingSession,
    currentProjectParam,
  } from './store';

  let { compact = false }: { compact?: boolean } = $props();

  function label(): string {
    const st = $sessionSaveState;
    switch (st) {
      case 'ensuring':
        return 'Session…';
      case 'ready':
        return 'Synced';
      case 'saving':
        return 'Saving…';
      case 'saved':
        return 'Saved';
      case 'conflict':
        return 'Conflict';
      case 'error':
        return 'Save error';
      default:
        return getCodingSessionId() ? 'Session' : 'No session';
    }
  }

  function title(): string {
    const m = $codingSessionMeta;
    const parts = [
      label(),
      m ? `id ${m.session_id.slice(0, 8)}…` : null,
      $codingSessionRevision != null ? `rev ${$codingSessionRevision}` : null,
      m?.draft_mode ? 'draft' : null,
      $sessionSaveDetail,
    ].filter(Boolean);
    return parts.join(' · ');
  }

  async function onClick() {
    const p = currentProjectParam();
    if (p) await ensureCodingSession(p);
  }
</script>

<button
  type="button"
  class="session-chip"
  class:compact
  class:ready={$sessionSaveState === 'ready' || $sessionSaveState === 'saved'}
  class:saving={$sessionSaveState === 'saving' || $sessionSaveState === 'ensuring'}
  class:conflict={$sessionSaveState === 'conflict'}
  class:error={$sessionSaveState === 'error'}
  title={title()}
  onclick={onClick}
>
  <span class="dot" aria-hidden="true"></span>
  {#if !compact}
    <span class="text">{label()}</span>
    {#if $codingSessionRevision != null && ($sessionSaveState === 'ready' || $sessionSaveState === 'saved')}
      <span class="rev">r{$codingSessionRevision}</span>
    {/if}
  {/if}
</button>

<style>
  .session-chip {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    border: 1px solid color-mix(in srgb, var(--border, #334155) 80%, transparent);
    background: color-mix(in srgb, var(--panel, #1e293b) 90%, transparent);
    color: var(--muted, #94a3b8);
    border-radius: 999px;
    padding: 0.15rem 0.55rem;
    font-size: 0.7rem;
    font-weight: 500;
    letter-spacing: 0.02em;
    cursor: pointer;
    line-height: 1.2;
    max-width: 12rem;
  }
  .session-chip.compact {
    padding: 0.2rem;
    border-radius: 999px;
  }
  .session-chip:hover {
    border-color: color-mix(in srgb, var(--accent, #38bdf8) 50%, var(--border, #334155));
  }
  .dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    background: #64748b;
    flex-shrink: 0;
  }
  .ready .dot {
    background: #22c55e;
    box-shadow: 0 0 0 2px color-mix(in srgb, #22c55e 25%, transparent);
  }
  .saving .dot {
    background: #eab308;
    animation: pulse 1s ease-in-out infinite;
  }
  .conflict .dot,
  .error .dot {
    background: #ef4444;
  }
  .conflict {
    color: #fca5a5;
  }
  .error {
    color: #fca5a5;
  }
  .saved.ready .dot,
  .session-chip.ready.saved .dot {
    background: #22c55e;
  }
  .text {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rev {
    opacity: 0.7;
    font-variant-numeric: tabular-nums;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }
</style>
