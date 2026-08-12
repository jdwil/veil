<script lang="ts">
  /**
   * PR Wizard — kick-ass structural code review.
   * Walk each DiffItem: agent rationale, before/after, approve or feedback
   * (send now vs queue). History + finalize approve/merge or request changes.
   */
  import { onMount } from 'svelte';
  import {
    focusConstructByName,
    currentProjectParam,
    codingSessionMeta,
  } from '$lib/ide/store';
  import {
    type PullRequest,
    type DiffItem,
    type ReviewComment,
    type WizardItemState,
    type QueuedFeedback,
    type StructDiff,
    type ConstructPeek,
    closePrWizard,
    prWizardChangeId,
    loadWizardDiff,
    fetchPullRequestDetail,
    fetchOpenPullRequests,
    rationalesFromPrTexts,
    buildWizardItems,
    refreshWizardRationales,
    prBelongsToProject,
    isSmokeOrFixturePr,
    itemDisplayName,
    itemKindLabel,
    itemKindClass,
    containerLabel,
    postReviewItem,
    finalizeWizardApi,
    mergeChangeApi,
    createAndSubmitPr,
    sendFeedbackToAgent,
    pathOf,
  } from '$lib/ide/prWizard';
  import { publishPrWizardViewport } from '$lib/ide/ideViewport';
  import { agentIsStreaming } from '$lib/agent/runtimeAgentSession';
  import { agentActive } from '$lib/ide/store';

  type Phase = 'loading' | 'pick' | 'walk' | 'summary' | 'history' | 'done' | 'error';

  let phase = $state<Phase>('loading');
  let error = $state<string | null>(null);
  let busy = $state(false);

  let prId = $state<string | null>(null);
  let pr = $state<PullRequest | null>(null);
  let comments = $state<ReviewComment[]>([]);
  let openPrs = $state<PullRequest[]>([]);
  let diff = $state<StructDiff | null>(null);
  let diffSource = $state<'pr' | 'working-tree' | 'pr-empty'>('working-tree');
  let diffNote = $state<string | null>(null);
  let items = $state<WizardItemState[]>([]);
  let step = $state(0);
  let feedbackDraft = $state('');
  let showFeedback = $state(false);
  let statusMsg = $state<string | null>(null);
  /** PRs for other projects / smoke tests (collapsed) */
  let otherPrs = $state<PullRequest[]>([]);

  const current = $derived(items[step] ?? null);
  const approvedCount = $derived(items.filter((i) => i.decision === 'approve').length);
  const feedbackCount = $derived(items.filter((i) => i.decision === 'feedback').length);
  const pendingCount = $derived(items.filter((i) => i.decision == null).length);
  const progressPct = $derived(
    items.length === 0 ? 0 : Math.round(((step + (current?.decision ? 1 : 0)) / items.length) * 100)
  );

  const meta = $derived($codingSessionMeta as Record<string, unknown> | null);
  const branchName = $derived(
    (meta?.branch_name as string) || (meta?.draft_mode ? 'work' : 'main')
  );

  function onKey(e: KeyboardEvent) {
    if (phase !== 'walk' || busy) return;
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (e.key === 'Escape') {
      e.preventDefault();
      closePrWizard();
      return;
    }
    if (e.key === 'a' || e.key === 'A') {
      e.preventDefault();
      if (current?.decision === 'approve') void clearDecision();
      else void decide('approve');
    } else if (e.key === 'u' || e.key === 'U') {
      e.preventDefault();
      if (current?.decision) void clearDecision();
    } else if (e.key === 'f' || e.key === 'F') {
      e.preventDefault();
      showFeedback = true;
    } else if (e.key === 'n' || e.key === 'N' || e.key === 'ArrowRight') {
      e.preventDefault();
      if (current?.decision) advance();
      else skip();
    } else if (e.key === 'p' || e.key === 'P' || e.key === 'ArrowLeft') {
      e.preventDefault();
      goStep(step - 1);
    } else if (e.key === 's' || e.key === 'S') {
      e.preventDefault();
      if (current?.decision === 'skip') void clearDecision();
      else skip();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', onKey);
    void bootstrap();
    return () => {
      window.removeEventListener('keydown', onKey);
      publishPrWizardViewport({ open: false });
    };
  });

  $effect(() => {
    // react if parent sets a different change id while open
    const id = $prWizardChangeId;
    if (id !== prId && phase !== 'loading') {
      prId = id;
      void bootstrap();
    }
  });

  /** Keep SessionFocus.panes in sync so the agent can discuss the current step. */
  $effect(() => {
    const cur = items[step] ?? null;
    const item = cur?.item;
    publishPrWizardViewport({
      open: true,
      phase,
      prId,
      prTitle: pr?.title ?? null,
      prStatus: pr?.status ?? null,
      sourceBranch: pr?.source_branch ?? null,
      targetBranch: pr?.target_branch ?? null,
      diffSource,
      step: phase === 'walk' ? step : undefined,
      total: items.length || undefined,
      itemName: item ? itemDisplayName(item) : null,
      itemKind: item ? itemKindLabel(item.kind) : null,
      itemPath: item ? pathOf(item) : null,
      itemSubkind: item?.subkind ?? null,
      itemNodeKind: item?.node_kind ?? null,
      container: item ? containerLabel(item) : null,
      signature: cur?.peek?.signature ?? null,
      rationale: cur?.rationale ?? null,
      decision: cur?.decision ?? null,
    });
  });

  async function bootstrap() {
    phase = 'loading';
    error = null;
    statusMsg = null;
    prId = $prWizardChangeId;
    try {
      if (prId) {
        await loadPr(prId);
      } else {
        // Offer open PRs or start session review
        try {
          const all = await fetchOpenPullRequests();
          const slug = currentProjectParam();
          const open = all.filter((p) =>
            ['Draft', 'ReadyForReview', 'ChangesRequested', 'Approved'].includes(p.status)
          );
          // Primary list: this project only, exclude smoke/fixture titles
          openPrs = open.filter(
            (p) => prBelongsToProject(p, slug) && !isSmokeOrFixturePr(p)
          );
          otherPrs = open.filter(
            (p) => !prBelongsToProject(p, slug) || isSmokeOrFixturePr(p)
          );
        } catch {
          openPrs = [];
          otherPrs = [];
        }
        // No project PRs → go straight to live working-tree (normal agent-edit path).
        // Still show chooser if project PRs exist OR only smoke/other (so they can pick working tree).
        if (openPrs.length > 0 || otherPrs.length > 0) {
          phase = 'pick';
          return;
        }
        await loadSessionReview();
      }
    } catch (e) {
      error = String(e);
      phase = 'error';
    }
  }

  async function loadPr(id: string) {
    prId = id;
    const detail = await fetchPullRequestDetail(id);
    pr = detail.pr;
    comments = detail.comments;
    await loadDiffAndItems(id, detail.pr.description || '');
    step = 0;
    // Already approved/merged → landing pad (merge / history), not a 100-item re-walk
    if (pr.status === 'Approved' || pr.status === 'Merged') {
      statusMsg =
        pr.status === 'Approved'
          ? 'This PR is already approved. Merge when ready, or re-walk structural changes below.'
          : 'This PR is already merged.';
      phase = 'done';
      return;
    }
    // Empty PR snapshot — don't pretend 100 live-tree items need approval
    if (diffSource === 'pr-empty' || items.length === 0) {
      phase = 'summary';
      return;
    }
    phase = 'walk';
  }

  async function loadSessionReview() {
    pr = null;
    prId = null;
    comments = [];
    await loadDiffAndItems(null, '');
    phase = items.length === 0 ? 'summary' : 'walk';
    step = 0;
  }

  async function loadDiffAndItems(id: string | null, description: string) {
    const slug = currentProjectParam();
    const { diff: d, source, note } = await loadWizardDiff({
      prId: id,
      slug,
      allowWorkingTreeFallback: !id,
    });
    diff = d;
    diffSource = source;
    diffNote = note ?? null;
    const rationales = rationalesFromPrTexts(description, comments);
    items = buildWizardItems(d.items || [], rationales, d);
    // Working-tree intent cache (write_source.rationales) may post-date PR open —
    // always merge live annotations so mid-review agent turns light up.
    if (items.some((it) => !it.rationale)) {
      try {
        const refreshed = await refreshWizardRationales({
          prId: id,
          slug,
          description,
          comments,
          items,
        });
        if (refreshed.applied > 0) items = refreshed.items;
      } catch {
        /* keep initial items */
      }
    }
  }

  let refreshingRationales = $state(false);
  let rationaleRefreshMsg = $state<string | null>(null);

  /** Re-pull write_source / PR-text intents without resetting walk decisions. */
  async function refreshRationalesNow(reason?: string) {
    if (!items.length || refreshingRationales) return;
    refreshingRationales = true;
    try {
      const { items: next, applied } = await refreshWizardRationales({
        prId,
        slug: currentProjectParam(),
        description: pr?.description || '',
        comments,
        items,
      });
      items = next;
      if (applied > 0) {
        rationaleRefreshMsg = `Loaded ${applied} agent rationale(s)${reason ? ` (${reason})` : ''}.`;
      } else if (reason === 'manual') {
        rationaleRefreshMsg = 'No new rationales on the server yet.';
      }
    } catch (e) {
      if (reason === 'manual') rationaleRefreshMsg = String(e);
    } finally {
      refreshingRationales = false;
    }
  }

  // After an agent turn finishes, re-merge intents (write_source.rationales is
  // process-local — wizard must re-fetch /diff; it does not auto-push into open UI).
  let wasAgentStreaming = false;
  $effect(() => {
    const streaming = $agentIsStreaming;
    if (wasAgentStreaming && !streaming && phase === 'walk' && items.length) {
      void refreshRationalesNow('agent turn');
      // Also re-pull PR comments (agent_reply may list construct rationales).
      if (prId) {
        void fetchPullRequestDetail(prId)
          .then((d) => {
            comments = d.comments;
            pr = d.pr;
            return refreshRationalesNow('agent reply');
          })
          .catch(() => {});
      }
    }
    wasAgentStreaming = streaming;
  });

  // Mid-turn write_source often lands before streaming ends — catch SSE activity.
  let wasAgentActive = false;
  $effect(() => {
    const active = $agentActive;
    if (wasAgentActive && !active && phase === 'walk' && items.length) {
      void refreshRationalesNow('source write');
    }
    wasAgentActive = active;
  });

  let jumpMsg = $state<string | null>(null);

  function jumpConstruct(item: DiffItem) {
    const name = item.name || item.to_name;
    if (!name) {
      jumpMsg = 'No construct name on this change.';
      return;
    }
    const ok = focusConstructByName(name);
    if (ok) {
      jumpMsg = `Selected “${name}” in the outline/canvas (left). Drag the wizard’s left edge if you need more canvas.`;
    } else {
      jumpMsg = `Could not find “${name}” in the current IR (wrong file active?). Select the package file and try again.`;
    }
  }

  function startWalkFromDone() {
    if (items.length === 0) {
      phase = 'summary';
      return;
    }
    step = 0;
    phase = 'walk';
  }

  async function decide(decision: 'approve' | 'feedback', opts?: { sendNow?: boolean }) {
    const cur = current;
    if (!cur || busy) return;
    // Already decided the same way — don't re-post; allow navigation only.
    if (decision === 'approve' && cur.decision === 'approve') {
      advance();
      return;
    }
    busy = true;
    error = null;
    try {
      if (decision === 'feedback' && !feedbackDraft.trim()) {
        showFeedback = true;
        busy = false;
        return;
      }
      items = items.map((it, i) =>
        i === step
          ? {
              ...it,
              decision,
              feedback: decision === 'feedback' ? feedbackDraft.trim() : it.feedback,
            }
          : it
      );
      const updated = items[step];

      if (prId) {
        await postReviewItem(prId, {
          decision,
          construct_path: pathOf(updated.item),
          body: decision === 'feedback' ? updated.feedback : 'Approved in PR Wizard.',
          send_now: !!opts?.sendNow,
          item_index: step,
          item_kind: updated.item.kind,
          item_name: itemDisplayName(updated.item),
          rationale: updated.rationale || undefined,
        });
        // refresh history
        try {
          const d = await fetchPullRequestDetail(prId);
          comments = d.comments;
        } catch {
          /* ignore */
        }
      }

      if (decision === 'feedback' && opts?.sendNow) {
        sendFeedbackToAgent(
          [
            {
              index: step,
              path: pathOf(updated.item),
              name: itemDisplayName(updated.item),
              kind: updated.item.kind,
              text: updated.feedback,
              rationale: updated.rationale,
            },
          ],
          pr?.title
        );
        items = items.map((it, i) => (i === step ? { ...it, sentToAgent: true } : it));
      }

      feedbackDraft = '';
      showFeedback = false;
      advance();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Undo approve / feedback / skip — item becomes pending again. */
  async function clearDecision() {
    const cur = current;
    if (!cur || busy || cur.decision == null) return;
    const prev = cur.decision;
    busy = true;
    error = null;
    try {
      items = items.map((it, i) =>
        i === step
          ? {
              ...it,
              decision: null,
              feedback: prev === 'feedback' ? '' : it.feedback,
              sentToAgent: false,
            }
          : it
      );
      feedbackDraft = '';
      showFeedback = false;
      if (prId && (prev === 'approve' || prev === 'feedback')) {
        await postReviewItem(prId, {
          decision: 'clear',
          construct_path: pathOf(cur.item),
          body: `Cleared ${prev} decision in PR Wizard.`,
          item_index: step,
          item_kind: cur.item.kind,
          item_name: itemDisplayName(cur.item),
        });
        try {
          const d = await fetchPullRequestDetail(prId);
          comments = d.comments;
        } catch {
          /* ignore */
        }
      }
      statusMsg = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function skip() {
    if (current?.decision === 'skip') {
      advance();
      return;
    }
    items = items.map((it, i) => (i === step ? { ...it, decision: 'skip' } : it));
    advance();
  }

  function advance() {
    if (step < items.length - 1) {
      step += 1;
      const next = items[step];
      feedbackDraft = next?.feedback || '';
      showFeedback = false;
    } else {
      phase = 'summary';
    }
  }

  function goStep(i: number) {
    if (i < 0 || i >= items.length) return;
    step = i;
    phase = 'walk';
    feedbackDraft = items[i]?.feedback || '';
    showFeedback = items[i]?.decision === 'feedback';
  }

  function queuedFeedback(): QueuedFeedback[] {
    return items
      .filter((i) => i.decision === 'feedback' && i.feedback.trim())
      .map((i) => ({
        index: i.index,
        path: pathOf(i.item),
        name: itemDisplayName(i.item),
        kind: i.item.kind,
        text: i.feedback,
        rationale: i.rationale,
      }));
  }

  async function sendAllFeedback() {
    const q = queuedFeedback();
    if (!q.length) return;
    sendFeedbackToAgent(q, pr?.title);
    statusMsg = `Sent ${q.length} feedback item(s) to the agent.`;
  }

  async function finalize(outcome: 'all_approved' | 'needs_work') {
    busy = true;
    error = null;
    try {
      let id = prId;
      if (!id) {
        // Create PR from session review so history is durable
        const title =
          diff && (diff.added || diff.removed || diff.changed)
            ? `Review: +${diff.added} −${diff.removed} ~${diff.changed} on ${branchName}`
            : `Review: ${branchName}`;
        const descParts = [
          'Opened from IDE PR Wizard (session working tree).',
          '',
          '## Changes',
          ...items.map(
            (it) =>
              `- **${itemDisplayName(it.item)}** (${it.item.kind}): ${it.decision || 'pending'}${
                it.rationale ? ` — ${it.rationale.slice(0, 120)}` : ''
              }`
          ),
        ];
        const created = await createAndSubmitPr({
          title,
          description: descParts.join('\n'),
          source_branch: branchName,
        });
        id = created.id;
        prId = id;
        pr = created;
      }

      // Persist any decisions not yet posted (session-first path)
      for (const it of items) {
        if (it.decision === 'approve' || it.decision === 'feedback') {
          try {
            await postReviewItem(id, {
              decision: it.decision,
              construct_path: pathOf(it.item),
              body:
                it.decision === 'feedback'
                  ? it.feedback || 'Needs work'
                  : 'Approved in PR Wizard.',
              item_index: it.index,
              item_kind: it.item.kind,
              item_name: itemDisplayName(it.item),
              rationale: it.rationale || undefined,
            });
          } catch {
            /* continue */
          }
        }
      }

      const summary =
        outcome === 'all_approved'
          ? `PR Wizard: approved ${approvedCount} structural change(s).`
          : `PR Wizard: ${approvedCount} approved, ${feedbackCount} need work.\n\n` +
            queuedFeedback()
              .map((q) => `- ${q.name}: ${q.text}`)
              .join('\n');

      await finalizeWizardApi(id, {
        outcome,
        summary,
        approved_count: approvedCount,
        feedback_count: feedbackCount,
      });

      if (outcome === 'needs_work') {
        const q = queuedFeedback();
        if (q.length) sendFeedbackToAgent(q, pr?.title);
        statusMsg = 'Changes requested — feedback sent to the agent.';
        phase = 'done';
      } else {
        statusMsg = 'All changes approved. Ready to merge.';
        phase = 'done';
      }

      try {
        const d = await fetchPullRequestDetail(id);
        pr = d.pr;
        comments = d.comments;
      } catch {
        /* ignore */
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function doMerge() {
    if (!prId) return;
    busy = true;
    error = null;
    try {
      await mergeChangeApi(prId, currentProjectParam());
      statusMsg = 'Merged to main. Product base updated.';
      if (pr) pr = { ...pr, status: 'Merged' };
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function beforeLines(item: DiffItem): string[] {
    if (item.before_preview?.length) return item.before_preview;
    if (typeof item.before === 'string') return item.before.split('\n').slice(0, 12);
    if (Array.isArray(item.before)) return item.before.map(String).slice(0, 12);
    return [];
  }
  function afterLines(item: DiffItem): string[] {
    if (item.after_preview?.length) return item.after_preview;
    if (typeof item.after === 'string') return item.after.split('\n').slice(0, 12);
    if (Array.isArray(item.after)) return item.after.map(String).slice(0, 12);
    return [];
  }

  function isWizardComment(c: ReviewComment): boolean {
    return (c.body || '').includes('[pr-wizard:');
  }

  function commentDecision(c: ReviewComment): string {
    const m = (c.body || '').match(/\[pr-wizard:(\w+)\]/);
    return m?.[1] || 'comment';
  }

  function isAgentReply(c: ReviewComment): boolean {
    return (c.body || '').includes('[pr-wizard:agent_reply]') || c.author === 'agent';
  }

  function statusLabel(s: string): string {
    switch (s) {
      case 'Draft':
        return 'Draft — not submitted for review yet';
      case 'ReadyForReview':
        return 'Ready for review';
      case 'ChangesRequested':
        return 'Changes requested';
      case 'Approved':
        return 'Approved — can merge';
      case 'Merged':
        return 'Merged';
      default:
        return s;
    }
  }

  function statusHint(s: string): string {
    switch (s) {
      case 'Draft':
        return 'Agent or human created this PR but may not have finished submitting it.';
      case 'ReadyForReview':
        return 'Walk structural changes, then approve or request changes.';
      case 'ChangesRequested':
        return 'Feedback was left earlier — re-review after the agent updates.';
      case 'Approved':
        return 'Human already approved. Open to merge, or review history.';
      default:
        return '';
    }
  }

  function peekHasContent(p: ConstructPeek): boolean {
    return !!(
      p.signature ||
      (p.fields && p.fields.length) ||
      (p.methods && p.methods.length) ||
      (p.body_preview && p.body_preview.length) ||
      (p.annotations && p.annotations.length)
    );
  }
</script>

{#snippet peekCard(p: ConstructPeek, label: string)}
  <div class="peek-card" class:base={p.side === 'base'} class:head={p.side === 'head'}>
    <div class="peek-h">
      <span class="peek-label">{label}</span>
      <strong>{p.name}</strong>
      {#if p.subkind}
        <span class="pill sm">{p.subkind}</span>
      {:else}
        <span class="dim">{p.node_kind}</span>
      {/if}
    </div>
    {#if p.path}
      <p class="peek-path"><code>{p.path}</code></p>
    {/if}
    {#if p.intent}
      <p class="peek-intent">{p.intent}</p>
    {/if}
    {#if p.signature}
      <div class="peek-block">
        <div class="peek-block-h">Signature</div>
        <pre>{p.signature}</pre>
      </div>
    {/if}
    {#if p.fields && p.fields.length}
      <div class="peek-block">
        <div class="peek-block-h">Fields</div>
        <ul>
          {#each p.fields as f}
            <li><code>{f}</code></li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if p.methods && p.methods.length}
      <div class="peek-block">
        <div class="peek-block-h">Methods</div>
        <ul>
          {#each p.methods as m}
            <li><code>{m}</code></li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if p.body_preview && p.body_preview.length}
      <div class="peek-block">
        <div class="peek-block-h">Body</div>
        {#each p.body_preview as line}
          <div class="line dim-line">{line}</div>
        {/each}
      </div>
    {/if}
    {#if p.annotations && p.annotations.length}
      <div class="peek-block">
        <div class="peek-block-h">Annotations</div>
        <p class="ann">{p.annotations.join(' · ')}</p>
      </div>
    {/if}
    {#if !peekHasContent(p)}
      <p class="muted">Construct present in IR — open in IDE for full detail.</p>
    {/if}
  </div>
{/snippet}

<div class="prw" role="complementary" aria-label="PR Wizard">
  <header class="prw-head">
    <div class="titles">
      <h2>PR Wizard</h2>
      <p class="sub">
        {#if pr}
          <span class="pill">{pr.status}</span>
          <strong>{pr.title}</strong>
          <span class="dim" title="Source branch → target branch"
            >⎇ {pr.source_branch} → {pr.target_branch}</span
          >
        {:else}
          <span class="pill">Live</span>
          Working tree on <code>{branchName}</code>
          {#if currentProjectParam()}
            <span class="dim">· {currentProjectParam()}</span>
          {/if}
        {/if}
      </p>
      {#if diffNote}
        <p class="banner-warn">{diffNote}</p>
      {/if}
    </div>
    <div class="head-actions">
      {#if phase === 'walk' || phase === 'summary' || phase === 'done'}
        <button
          type="button"
          class="ghost"
          class:active={phase === 'history'}
          onclick={() => (phase = phase === 'history' ? 'walk' : 'history')}
        >
          History
          {#if comments.length}
            <span class="badge">{comments.length}</span>
          {/if}
        </button>
      {/if}
      <button type="button" class="close" onclick={() => closePrWizard()} aria-label="Close">
        ✕
      </button>
    </div>
  </header>

  {#if phase === 'walk' && items.length > 0}
    <div class="progress" aria-hidden="true">
      <div class="bar" style="width: {Math.max(4, ((step + 1) / items.length) * 100)}%"></div>
    </div>
    <div class="step-meta">
      <span>Change {step + 1} of {items.length}</span>
      <span class="counts">
        <span class="ok">{approvedCount} ✓</span>
        <span class="fb">{feedbackCount} 💬</span>
        <span class="pend">{pendingCount} left</span>
      </span>
      <span class="keys dim" title="Keyboard"
        >A approve · U undo · F feedback · N next · P prev · Esc close</span
      >
    </div>
  {/if}

  <div class="prw-body">
    {#if phase === 'loading'}
      <p class="muted center">Loading structural changes…</p>
    {:else if phase === 'error'}
      <p class="err">{error}</p>
      <button type="button" class="btn" onclick={() => void bootstrap()}>Retry</button>
    {:else if phase === 'pick'}
      <div class="pick-intro">
        <h3>What do you want to review?</h3>
        <p class="muted">
          The PR Wizard walks <strong>structural changes</strong> (added/removed/modified constructs)
          one at a time so you can approve or send feedback to the agent.
          Nothing lands on <code>main</code> until you explicitly merge an <em>Approved</em> PR.
        </p>
      </div>

      <!-- Primary: live edits in this IDE session -->
      <section class="pick-section">
        <h4>This IDE session</h4>
        <p class="muted sm">
          Uncommitted or recently edited work on branch
          <code>{branchName}</code>
          {#if currentProjectParam()}
            in project <code>{currentProjectParam()}</code>
          {/if}.
          Use this after the agent just finished editing — even if no PR exists yet.
        </p>
        <button type="button" class="btn primary pick-primary" onclick={() => void loadSessionReview()}>
          Review current working tree
        </button>
      </section>

      <!-- Secondary: formal pull requests for THIS project -->
      <section class="pick-section">
        <h4>
          PRs for
          {#if currentProjectParam()}
            <code>{currentProjectParam()}</code>
          {:else}
            this project
          {/if}
          ({openPrs.length})
        </h4>
        <p class="muted sm">
          Pull requests are <strong>platform-wide SDLC records</strong>, not VEIL packages.
          The PR Wizard is part of the <strong>core IDE</strong>; these cards are optional formal
          reviews. <code>work</code> / <code>pr-wizard-test</code> are <strong>git-shaped branch
          names</strong>, not project names.
        </p>
        {#if openPrs.length === 0}
          <p class="muted sm">No open PRs linked to this project. Use the working-tree review above.</p>
        {:else}
          <ul class="pr-list">
            {#each openPrs as p}
              <li>
                <button type="button" class="pr-row" onclick={() => void loadPr(p.id)}>
                  <div class="pr-row-main">
                    <span class="pill sm" title={statusHint(p.status)}>{p.status}</span>
                    <span class="t">{p.title}</span>
                  </div>
                  <div class="pr-row-meta">
                    <span class="meta-label">branch</span>
                    <code>{p.source_branch || '—'}</code>
                    <span class="meta-arrow">→</span>
                    <code>{p.target_branch || 'main'}</code>
                    <span class="status-desc">{statusLabel(p.status)}</span>
                  </div>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      {#if otherPrs.length > 0}
        <details class="other-prs">
          <summary>
            Other / test PRs on the platform ({otherPrs.length}) — usually ignore
          </summary>
          <p class="muted sm">
            Includes API smoke titles (“PR Wizard smoke”, “100pct smoke”) and PRs for other
            projects. Opening an <em>Approved</em> smoke PR will <strong>not</strong> dump your
            live Agent Registry tree for re-approval.
          </p>
          <ul class="pr-list">
            {#each otherPrs as p}
              <li>
                <button type="button" class="pr-row dim-row" onclick={() => void loadPr(p.id)}>
                  <div class="pr-row-main">
                    <span class="pill sm">{p.status}</span>
                    <span class="t">{p.title}</span>
                    {#if isSmokeOrFixturePr(p)}
                      <span class="pill sm">test</span>
                    {/if}
                  </div>
                  <div class="pr-row-meta">
                    <span class="meta-label">branch</span>
                    <code>{p.source_branch || '—'}</code>
                  </div>
                </button>
              </li>
            {/each}
          </ul>
        </details>
      {/if}
    {:else if phase === 'history'}
      <h3>PR history</h3>
      {#if comments.length === 0}
        <p class="muted">No comments yet. Decisions you make appear here.</p>
      {:else}
        <ul class="hist">
          {#each comments as c}
            <li class:wizard={isWizardComment(c)} class:agent={isAgentReply(c)}>
              <div class="hist-h">
                <strong>{c.author}</strong>
                {#if isWizardComment(c)}
                  <span class="pill sm">{commentDecision(c)}</span>
                {/if}
                {#if isAgentReply(c)}
                  <span class="pill sm">agent</span>
                {/if}
                {#if c.construct_path}
                  <code>{c.construct_path}</code>
                {/if}
                <span class="dim">{c.created_at?.slice(0, 19)}</span>
              </div>
              <pre class="hist-b">{c.body}</pre>
            </li>
          {/each}
        </ul>
      {/if}
      <button type="button" class="btn" onclick={() => (phase = items.length ? 'walk' : 'summary')}>
        Back to review
      </button>
    {:else if phase === 'walk' && current}
      {@const it = current.item}
      <div class="item-card" class:add={itemKindClass(it.kind) === 'add'} class:rem={itemKindClass(it.kind) === 'rem'} class:chg={itemKindClass(it.kind) === 'chg'}>
        <div class="item-top">
          <span class="kind-badge {itemKindClass(it.kind)}">{itemKindLabel(it.kind)}</span>
          <h3 class="item-name">{itemDisplayName(it)}</h3>
          {#if it.subkind}
            <span class="pill sm">{it.subkind}</span>
          {/if}
          {#if it.node_kind}
            <span class="dim">{it.node_kind}</span>
          {/if}
        </div>
        <p class="container">{containerLabel(it)}</p>
        <p class="path"><code>{pathOf(it)}</code></p>

        {#if current.rationale}
          <section class="rationale">
            <h4>Agent rationale</h4>
            <p>{current.rationale}</p>
          </section>
        {:else}
          <section class="rationale empty">
            <h4>Agent rationale</h4>
            <p class="muted">
              No intent on this step yet. Ask the agent for a rationale (it should
              pass <code>write_source</code> <code>rationales</code> or document
              why in the PR). Intent appears here automatically when attached.
            </p>
            <button
              type="button"
              class="btn sm"
              disabled={refreshingRationales}
              onclick={() => void refreshRationalesNow('manual')}
            >
              {refreshingRationales ? 'Refreshing…' : 'Refresh intent'}
            </button>
            {#if rationaleRefreshMsg}
              <p class="muted tiny">{rationaleRefreshMsg}</p>
            {/if}
          </section>
        {/if}

        <!-- Construct snapshot (what actually changed) -->
        {#if current.peek || current.peekBase}
          <section class="peek-section">
            <h4>Construct</h4>
            <div class="peek-grid" class:pair={!!(current.peek && current.peekBase)}>
              {#if current.peekBase}
                {@render peekCard(current.peekBase, 'Before')}
              {/if}
              {#if current.peek}
                {@render peekCard(
                  current.peek,
                  it.kind === 'removed' ? 'Removed' : it.kind === 'added' ? 'Added' : 'After'
                )}
              {/if}
            </div>
          </section>
        {:else if beforeLines(it).length || afterLines(it).length || it.kind === 'signature_changed'}
          <div class="diff-grid">
            <div class="col before">
              <div class="col-h">Before</div>
              {#if beforeLines(it).length}
                {#each beforeLines(it) as line}
                  <div class="line rem">− {line}</div>
                {/each}
              {:else}
                <p class="muted">—</p>
              {/if}
            </div>
            <div class="col after">
              <div class="col-h">After</div>
              {#if afterLines(it).length}
                {#each afterLines(it) as line}
                  <div class="line add">+ {line}</div>
                {/each}
              {:else}
                <p class="muted">—</p>
              {/if}
            </div>
          </div>
        {:else}
          <p class="note">
            No IR peek for this item (sparse baseline or parse failure). Use
            <strong>Show in IDE</strong> to open the live construct.
          </p>
        {/if}

        <div class="item-actions-row">
          <button type="button" class="btn" onclick={() => jumpConstruct(it)}>
            Show in IDE
          </button>
        </div>
        {#if jumpMsg}
          <p class="jump-msg">{jumpMsg}</p>
        {/if}

        {#if showFeedback}
          <div class="feedback-box">
            <label for="fb">What needs to change?</label>
            <textarea
              id="fb"
              rows="3"
              placeholder="e.g. Prefer agg Order over struct; keep bang contract on find!"
              bind:value={feedbackDraft}
            ></textarea>
            <div class="fb-actions">
              <button
                type="button"
                class="btn"
                disabled={busy || !feedbackDraft.trim()}
                onclick={() => void decide('feedback', { sendNow: false })}
              >
                Queue feedback
              </button>
              <button
                type="button"
                class="btn primary"
                disabled={busy || !feedbackDraft.trim()}
                onclick={() => void decide('feedback', { sendNow: true })}
              >
                Send to agent now
              </button>
              <button type="button" class="ghost" onclick={() => (showFeedback = false)}>Cancel</button>
            </div>
          </div>
        {/if}
      </div>

      <!-- step strip -->
      <div class="step-strip" role="tablist">
        {#each items as it, i}
          <button
            type="button"
            class="dot"
            class:current={i === step}
            class:ok={it.decision === 'approve'}
            class:fb={it.decision === 'feedback'}
            class:skip={it.decision === 'skip'}
            title="{itemDisplayName(it.item)} — {it.decision || 'pending'}"
            onclick={() => goStep(i)}
          ></button>
        {/each}
      </div>
    {:else if phase === 'summary' || (phase === 'walk' && items.length === 0)}
      <div class="summary">
        <h3>Review summary</h3>
        {#if items.length === 0}
          <p class="muted">
            No structural IR changes detected
            {#if diffSource === 'working-tree'}in the working tree{:else}on this PR{/if}.
            {#if diff?.description}
              <br />{diff.description}
            {/if}
          </p>
        {:else}
          <ul class="sum-list">
            <li><span class="ok">{approvedCount}</span> approved</li>
            <li><span class="fb">{feedbackCount}</span> need work</li>
            <li><span class="pend">{pendingCount}</span> skipped / pending</li>
          </ul>
          {#if feedbackCount > 0}
            <h4>Queued feedback</h4>
            <ul class="fb-list">
              {#each queuedFeedback() as q}
                <li>
                  <strong>{q.name}</strong>
                  <span class="dim">{q.kind}</span>
                  <p>{q.text}</p>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
        {#if error}
          <p class="err">{error}</p>
        {/if}
      </div>
    {:else if phase === 'done'}
      <div class="summary">
        <h3>{pr?.status === 'Approved' ? 'Approved PR' : 'Done'}</h3>
        {#if statusMsg}
          <p class="ok-msg">{statusMsg}</p>
        {/if}
        {#if pr}
          <p>
            <span class="pill">{pr.status}</span>
            <strong>{pr.title}</strong>
          </p>
          <p class="muted sm">
            Branch <code>{pr.source_branch}</code> → <code>{pr.target_branch || 'main'}</code>.
            This is a platform pull request, not a VEIL project.
          </p>
          {#if diffSource === 'pr-empty'}
            <p class="banner-warn">
              No structural snapshot on this PR’s branch — there is nothing to re-approve item-by-item.
            </p>
          {:else if items.length > 0}
            <p class="muted sm">
              Structural snapshot has {items.length} item(s). Status is already
              <strong>{pr.status}</strong> — you do not need to approve each again.
            </p>
          {/if}
        {/if}
        {#if error}
          <p class="err">{error}</p>
        {/if}
      </div>
    {/if}
  </div>

  <footer class="prw-foot">
    {#if error && phase === 'walk'}
      <p class="err foot-err">{error}</p>
    {/if}
    {#if phase === 'walk' && current}
      <button type="button" class="ghost" disabled={step === 0} onclick={() => goStep(step - 1)}>
        ← Back
      </button>
      <div class="spacer"></div>
      {#if current.decision}
        <span class="decision-badge" class:ok={current.decision === 'approve'} class:fb={current.decision === 'feedback'} class:skip={current.decision === 'skip'}>
          {#if current.decision === 'approve'}Approved
          {:else if current.decision === 'feedback'}Feedback queued
          {:else}Skipped{/if}
        </span>
        <button
          type="button"
          class="ghost"
          disabled={busy}
          title="Clear decision (U)"
          onclick={() => void clearDecision()}
        >
          Undo
        </button>
        <button
          type="button"
          class="btn"
          disabled={busy}
          onclick={() => advance()}
        >
          {step < items.length - 1 ? 'Next →' : 'Summary →'}
        </button>
        {#if current.decision !== 'approve'}
          <button
            type="button"
            class="btn primary"
            disabled={busy}
            onclick={() => void decide('approve')}
          >
            Approve ✓
          </button>
        {/if}
        {#if current.decision !== 'feedback'}
          <button
            type="button"
            class="btn warn"
            disabled={busy}
            onclick={() => {
              showFeedback = true;
            }}
          >
            Request changes
          </button>
        {/if}
      {:else}
        <button type="button" class="ghost" disabled={busy} onclick={() => skip()}>Skip</button>
        <button
          type="button"
          class="btn warn"
          disabled={busy}
          onclick={() => {
            showFeedback = true;
          }}
        >
          Request changes
        </button>
        <button
          type="button"
          class="btn primary"
          disabled={busy}
          onclick={() => void decide('approve')}
        >
          Approve ✓
        </button>
      {/if}
    {:else if phase === 'summary'}
      <button type="button" class="ghost" onclick={() => closePrWizard()}>Close</button>
      <div class="spacer"></div>
      {#if feedbackCount > 0}
        <button type="button" class="btn" disabled={busy} onclick={() => void sendAllFeedback()}>
          Send all to agent
        </button>
        <button
          type="button"
          class="btn warn"
          disabled={busy}
          onclick={() => void finalize('needs_work')}
        >
          Request changes
        </button>
      {/if}
      {#if pendingCount === 0 && feedbackCount === 0 && approvedCount > 0}
        <button
          type="button"
          class="btn primary"
          disabled={busy}
          onclick={() => void finalize('all_approved')}
        >
          Approve PR
        </button>
      {:else if items.length === 0}
        <button
          type="button"
          class="btn primary"
          disabled={busy}
          onclick={() => void finalize('all_approved')}
        >
          Open PR (empty diff)
        </button>
      {:else if approvedCount > 0 && feedbackCount === 0}
        <button
          type="button"
          class="btn primary"
          disabled={busy}
          onclick={() => void finalize('all_approved')}
        >
          Approve reviewed ({approvedCount})
        </button>
      {/if}
    {:else if phase === 'done'}
      <button type="button" class="ghost" onclick={() => closePrWizard()}>Close</button>
      <div class="spacer"></div>
      {#if items.length > 0 && diffSource !== 'pr-empty'}
        <button type="button" class="btn" onclick={() => startWalkFromDone()}>
          Browse {items.length} structural items
        </button>
      {/if}
      {#if pr?.status === 'Approved' && prId}
        <button type="button" class="btn primary" disabled={busy} onclick={() => void doMerge()}>
          Merge to {pr.target_branch || 'main'}
        </button>
      {/if}
      {#if pr?.status === 'ChangesRequested'}
        <button type="button" class="btn" disabled={busy} onclick={() => void sendAllFeedback()}>
          Re-send feedback to agent
        </button>
      {/if}
    {:else if phase === 'pick'}
      <button type="button" class="ghost" onclick={() => closePrWizard()}>Close</button>
    {/if}
  </footer>
</div>

<style>
  .prw {
    /* Fills the right-rail overlay from IdeApp — never covers outline/canvas */
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    background:
      linear-gradient(165deg, rgba(24, 24, 27, 0.98) 0%, rgba(9, 9, 11, 0.99) 100%);
    color: var(--veil-text, #e5e5e5);
    border-left: 2px solid rgba(59, 130, 246, 0.4);
    box-shadow: -8px 0 40px rgba(0, 0, 0, 0.45);
  }
  .banner-warn {
    margin: 0.4rem 0 0;
    padding: 0.45rem 0.55rem;
    border-radius: 6px;
    font-size: 0.78rem;
    line-height: 1.4;
    color: #fbbf24;
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.3);
  }
  .other-prs {
    margin: 0.75rem 0 1rem;
    padding: 0.65rem 0.75rem;
    border-radius: 8px;
    border: 1px dashed rgba(115, 115, 115, 0.4);
    font-size: 0.85rem;
  }
  .other-prs summary {
    cursor: pointer;
    color: #a3a3a3;
    font-weight: 600;
  }
  .dim-row {
    opacity: 0.85;
  }
  .peek-section {
    margin: 0.75rem 0;
  }
  .peek-section h4 {
    margin: 0 0 0.45rem;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: #a3a3a3;
  }
  .peek-grid {
    display: grid;
    grid-template-columns: 1fr;
    gap: 0.55rem;
  }
  .peek-grid.pair {
    grid-template-columns: 1fr 1fr;
  }
  @media (max-width: 900px) {
    .peek-grid.pair {
      grid-template-columns: 1fr;
    }
  }
  .peek-card {
    border-radius: 10px;
    border: 1px solid rgba(115, 115, 115, 0.35);
    padding: 0.65rem 0.75rem;
    background: rgba(0, 0, 0, 0.25);
  }
  .peek-card.head {
    border-color: rgba(34, 197, 94, 0.35);
  }
  .peek-card.base {
    border-color: rgba(248, 113, 113, 0.3);
  }
  .peek-h {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem;
    margin-bottom: 0.35rem;
  }
  .peek-label {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    color: #737373;
  }
  .peek-path {
    margin: 0 0 0.35rem;
    font-size: 0.72rem;
  }
  .peek-intent {
    margin: 0 0 0.5rem;
    font-size: 0.84rem;
    color: #93c5fd;
    font-style: italic;
  }
  .peek-block {
    margin-top: 0.4rem;
  }
  .peek-block-h {
    font-size: 0.65rem;
    font-weight: 650;
    text-transform: uppercase;
    color: #737373;
    margin-bottom: 0.2rem;
  }
  .peek-block pre {
    margin: 0;
    font-size: 0.72rem;
    white-space: pre-wrap;
    color: #d4d4d4;
  }
  .peek-block ul {
    margin: 0;
    padding-left: 1rem;
    font-size: 0.78rem;
  }
  .peek-block .ann {
    margin: 0;
    font-size: 0.75rem;
    color: #a3a3a3;
  }
  .dim-line {
    font-family: ui-monospace, monospace;
    font-size: 0.72rem;
    color: #a3a3a3;
    padding: 0.05rem 0;
  }
  .jump-msg {
    margin: 0.4rem 0 0;
    font-size: 0.78rem;
    color: #93c5fd;
  }
  .prw-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem 1.25rem 0.75rem;
    border-bottom: 1px solid rgba(115, 115, 115, 0.25);
  }
  .titles h2 {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .sub {
    margin: 0.35rem 0 0;
    font-size: 0.82rem;
    color: var(--veil-text-dim, #a3a3a3);
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .head-actions {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .close {
    border: none;
    background: transparent;
    color: var(--veil-text-dim, #a3a3a3);
    font-size: 1.1rem;
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    border-radius: 6px;
  }
  .close:hover {
    background: rgba(255, 255, 255, 0.06);
    color: #fff;
  }
  .progress {
    height: 3px;
    background: rgba(255, 255, 255, 0.06);
  }
  .progress .bar {
    height: 100%;
    background: linear-gradient(90deg, #22c55e, #3b82f6);
    transition: width 0.2s ease;
  }
  .step-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
    padding: 0.45rem 1.25rem;
    font-size: 0.75rem;
    color: var(--veil-text-dim, #a3a3a3);
    border-bottom: 1px solid rgba(115, 115, 115, 0.15);
  }
  .keys {
    font-size: 0.68rem;
    opacity: 0.85;
  }
  .counts {
    display: flex;
    gap: 0.75rem;
  }
  .ok {
    color: #4ade80;
  }
  .fb {
    color: #fbbf24;
  }
  .pend {
    color: #a3a3a3;
  }
  .prw-body {
    flex: 1;
    overflow-y: auto;
    padding: 1rem 1.25rem 1.25rem;
  }
  .prw-foot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.85rem 1.25rem;
    border-top: 1px solid rgba(115, 115, 115, 0.25);
    background: rgba(0, 0, 0, 0.25);
  }
  .spacer {
    flex: 1;
  }
  .decision-badge {
    font-size: 0.75rem;
    font-weight: 700;
    padding: 0.3rem 0.65rem;
    border-radius: 999px;
    border: 1px solid rgba(115, 115, 115, 0.4);
    color: #a3a3a3;
    white-space: nowrap;
  }
  .decision-badge.ok {
    color: #86efac;
    border-color: rgba(34, 197, 94, 0.45);
    background: rgba(34, 197, 94, 0.12);
  }
  .decision-badge.fb {
    color: #fbbf24;
    border-color: rgba(251, 191, 36, 0.45);
    background: rgba(251, 191, 36, 0.1);
  }
  .decision-badge.skip {
    color: #a3a3a3;
    background: rgba(115, 115, 115, 0.12);
  }
  .btn,
  .ghost {
    border-radius: 8px;
    font-size: 0.84rem;
    font-weight: 600;
    padding: 0.45rem 0.85rem;
    cursor: pointer;
    border: 1px solid transparent;
  }
  .btn {
    background: rgba(255, 255, 255, 0.08);
    color: #e5e5e5;
    border-color: rgba(115, 115, 115, 0.4);
  }
  .btn.primary {
    background: linear-gradient(180deg, #3b82f6, #2563eb);
    border-color: #1d4ed8;
    color: #fff;
  }
  .btn.warn {
    background: rgba(251, 191, 36, 0.12);
    border-color: rgba(251, 191, 36, 0.45);
    color: #fbbf24;
  }
  .btn:disabled,
  .ghost:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ghost {
    background: transparent;
    color: var(--veil-text-dim, #a3a3a3);
    border-color: transparent;
  }
  .ghost:hover:not(:disabled) {
    color: #fff;
    background: rgba(255, 255, 255, 0.05);
  }
  .ghost.active {
    color: #93c5fd;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
    font-size: 0.7rem;
    font-weight: 650;
    background: rgba(59, 130, 246, 0.15);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.35);
  }
  .pill.sm {
    font-size: 0.65rem;
  }
  .dim {
    color: #737373;
    font-size: 0.8rem;
  }
  .muted {
    color: #a3a3a3;
    font-size: 0.85rem;
    line-height: 1.45;
  }
  .center {
    text-align: center;
    margin-top: 2rem;
  }
  .err {
    color: #f87171;
    font-size: 0.85rem;
  }
  .foot-err {
    position: absolute;
    left: 1.25rem;
    bottom: 3.2rem;
  }
  .item-card {
    border-radius: 12px;
    border: 1px solid rgba(115, 115, 115, 0.35);
    padding: 1rem 1.1rem;
    background: rgba(255, 255, 255, 0.02);
  }
  .item-card.add {
    border-color: rgba(34, 197, 94, 0.4);
  }
  .item-card.rem {
    border-color: rgba(248, 113, 113, 0.4);
  }
  .item-card.chg {
    border-color: rgba(251, 191, 36, 0.4);
  }
  .item-top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.45rem;
  }
  .item-name {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 700;
  }
  .kind-badge {
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.15rem 0.4rem;
    border-radius: 4px;
  }
  .kind-badge.add {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
  }
  .kind-badge.rem {
    background: rgba(248, 113, 113, 0.15);
    color: #f87171;
  }
  .kind-badge.chg {
    background: rgba(251, 191, 36, 0.15);
    color: #fbbf24;
  }
  .container {
    margin: 0.5rem 0 0.15rem;
    font-size: 0.8rem;
    color: #a3a3a3;
  }
  .path {
    margin: 0 0 0.75rem;
    font-size: 0.78rem;
  }
  .path code {
    color: #93c5fd;
  }
  .rationale {
    margin: 0.75rem 0;
    padding: 0.65rem 0.75rem;
    border-radius: 8px;
    background: rgba(59, 130, 246, 0.08);
    border: 1px solid rgba(59, 130, 246, 0.25);
  }
  .rationale.empty {
    background: rgba(255, 255, 255, 0.02);
    border-color: rgba(115, 115, 115, 0.25);
  }
  .rationale h4 {
    margin: 0 0 0.35rem;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: #93c5fd;
  }
  .rationale p {
    margin: 0;
    font-size: 0.88rem;
    line-height: 1.45;
  }
  .rationale .btn.sm {
    margin-top: 0.5rem;
    padding: 0.25rem 0.55rem;
    font-size: 0.75rem;
  }
  .rationale .tiny {
    margin-top: 0.35rem;
    font-size: 0.72rem;
  }
  .diff-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.65rem;
    margin: 0.75rem 0;
  }
  @media (max-width: 720px) {
    .diff-grid {
      grid-template-columns: 1fr;
    }
  }
  .col {
    border-radius: 8px;
    border: 1px solid rgba(115, 115, 115, 0.3);
    overflow: hidden;
    font-size: 0.78rem;
  }
  .col-h {
    padding: 0.3rem 0.5rem;
    font-weight: 650;
    background: rgba(0, 0, 0, 0.3);
    color: #a3a3a3;
  }
  .line {
    padding: 0.15rem 0.5rem;
    font-family: ui-monospace, monospace;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .line.rem {
    background: rgba(248, 113, 113, 0.08);
    color: #fca5a5;
  }
  .line.add {
    background: rgba(34, 197, 94, 0.08);
    color: #86efac;
  }
  .block {
    margin: 0;
    padding: 0.5rem;
    white-space: pre-wrap;
    font-size: 0.75rem;
  }
  .note {
    font-size: 0.85rem;
    color: #a3a3a3;
  }
  .feedback-box {
    margin-top: 0.85rem;
    padding-top: 0.75rem;
    border-top: 1px solid rgba(115, 115, 115, 0.25);
  }
  .feedback-box label {
    display: block;
    font-size: 0.8rem;
    font-weight: 600;
    margin-bottom: 0.35rem;
  }
  .feedback-box textarea {
    width: 100%;
    box-sizing: border-box;
    border-radius: 8px;
    border: 1px solid rgba(115, 115, 115, 0.45);
    background: rgba(0, 0, 0, 0.35);
    color: #e5e5e5;
    padding: 0.55rem 0.65rem;
    font-size: 0.85rem;
    resize: vertical;
  }
  .fb-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.5rem;
  }
  .step-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 1rem;
    justify-content: center;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 1px solid rgba(115, 115, 115, 0.5);
    background: transparent;
    padding: 0;
    cursor: pointer;
  }
  .dot.current {
    outline: 2px solid #3b82f6;
    outline-offset: 2px;
  }
  .dot.ok {
    background: #22c55e;
    border-color: #22c55e;
  }
  .dot.fb {
    background: #fbbf24;
    border-color: #fbbf24;
  }
  .dot.skip {
    background: #525252;
  }
  .pick-intro h3 {
    margin: 0 0 0.5rem;
  }
  .pick-section {
    margin: 1.1rem 0 1.25rem;
    padding: 0.85rem 0.9rem;
    border-radius: 10px;
    border: 1px solid rgba(115, 115, 115, 0.3);
    background: rgba(0, 0, 0, 0.2);
  }
  .pick-section h4 {
    margin: 0 0 0.4rem;
    font-size: 0.9rem;
  }
  .muted.sm {
    font-size: 0.8rem;
    margin: 0 0 0.65rem;
    line-height: 1.45;
  }
  .pick-primary {
    width: 100%;
  }
  .pick-footnote {
    margin: 0.65rem 0 0;
  }
  .pr-list {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .pr-row {
    width: 100%;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.7rem 0.75rem;
    margin-bottom: 0.45rem;
    border-radius: 8px;
    border: 1px solid rgba(115, 115, 115, 0.35);
    background: rgba(255, 255, 255, 0.03);
    color: inherit;
    cursor: pointer;
  }
  .pr-row:hover {
    border-color: #3b82f6;
  }
  .pr-row-main {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    align-items: center;
  }
  .pr-row .t {
    font-weight: 600;
  }
  .pr-row-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    align-items: center;
    font-size: 0.75rem;
    color: #a3a3a3;
  }
  .meta-label {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    color: #737373;
  }
  .meta-arrow {
    opacity: 0.6;
  }
  .status-desc {
    width: 100%;
    font-size: 0.72rem;
    color: #737373;
  }
  .hist {
    list-style: none;
    padding: 0;
    margin: 0 0 1rem;
  }
  .hist li {
    padding: 0.65rem 0.75rem;
    border-radius: 8px;
    border: 1px solid rgba(115, 115, 115, 0.25);
    margin-bottom: 0.5rem;
    background: rgba(0, 0, 0, 0.2);
  }
  .hist li.wizard {
    border-color: rgba(59, 130, 246, 0.35);
  }
  .hist li.agent {
    border-color: rgba(167, 139, 250, 0.4);
    background: rgba(139, 92, 246, 0.06);
  }
  .hist-h {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    font-size: 0.8rem;
    margin-bottom: 0.35rem;
  }
  .hist-b {
    margin: 0;
    white-space: pre-wrap;
    font-size: 0.78rem;
    color: #d4d4d4;
    font-family: ui-monospace, monospace;
  }
  .summary h3 {
    margin: 0 0 0.75rem;
  }
  .sum-list {
    list-style: none;
    padding: 0;
    display: flex;
    gap: 1.25rem;
    font-size: 0.95rem;
    font-weight: 600;
  }
  .fb-list {
    padding-left: 1.1rem;
    font-size: 0.85rem;
  }
  .fb-list p {
    margin: 0.15rem 0 0.5rem;
    color: #a3a3a3;
  }
  .ok-msg {
    color: #4ade80;
    font-weight: 600;
  }
  .badge {
    display: inline-block;
    min-width: 1.1rem;
    padding: 0 0.3rem;
    border-radius: 999px;
    background: rgba(59, 130, 246, 0.3);
    font-size: 0.65rem;
    margin-left: 0.2rem;
  }
  .item-actions-row {
    margin-top: 0.5rem;
  }
</style>
