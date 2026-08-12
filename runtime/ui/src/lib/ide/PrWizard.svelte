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
    getCodingSessionId,
    ideRequestHeaders,
  } from '$lib/ide/store';
  import {
    type PullRequest,
    type DiffItem,
    type ReviewComment,
    type WizardItemState,
    type WizardGroup,
    type QueuedFeedback,
    type StructDiff,
    type ConstructPeek,
    type PreviewDepth,
    type WizardMode,
    type RiskLevel,
    type FileDiff,
    closePrWizard,
    prWizardChangeId,
    loadWizardDiff,
    fetchPullRequestDetail,
    fetchOpenPullRequests,
    rationalesFromPrTexts,
    buildWizardItems,
    buildWizardGroups,
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
    riskLabel,
    resolveReviewPresentation,
    postJournalEntry,
    fetchJournal,
    fetchLearnJournalWalk,
    fetchReviewPolicies,
    platformRoot,
    type LayerReviewPolicy,
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
  /** Flat items (decisions + API posts). */
  let items = $state<WizardItemState[]>([]);
  /** Grouped walk steps (risk-ordered). */
  let groups = $state<WizardGroup[]>([]);
  /** Index into `groups` (not flat items). */
  let step = $state(0);
  let feedbackDraft = $state('');
  let noteDraft = $state('');
  let showFeedback = $state(false);
  let showNote = $state(false);
  let showFileDiff = $state(false);
  let previewDepth = $state<PreviewDepth>('peek');
  let wizardMode = $state<WizardMode>('review');
  let learnEntries = $state<Record<string, unknown>[]>([]);
  let learnConstructEntries = $state<Record<string, unknown>[]>([]);
  let learnPrEntries = $state<Record<string, unknown>[]>([]);
  let learnCursor = $state(0);
  let hostReviewPolicies = $state<Record<string, LayerReviewPolicy>>({});
  let statusMsg = $state<string | null>(null);
  /** True after a successful all_approved finalize — always show Merge even if detail refresh lags. */
  let readyToMerge = $state(false);
  /** Approved PRs for this project (surfaced first on reopen). */
  let approvedPrs = $state<PullRequest[]>([]);
  /** PRs for other projects / smoke tests (collapsed) */
  let otherPrs = $state<PullRequest[]>([]);

  const currentGroup = $derived(groups[step] ?? null);
  /** Primary child for display (first in group). */
  const current = $derived(currentGroup?.children[0] ?? null);
  const approvedCount = $derived(items.filter((i) => i.decision === 'approve').length);
  const feedbackCount = $derived(items.filter((i) => i.decision === 'feedback').length);
  const pendingCount = $derived(items.filter((i) => i.decision == null).length);
  const progressPct = $derived(
    groups.length === 0
      ? 0
      : Math.round(((step + (currentGroup?.decision ? 1 : 0)) / groups.length) * 100)
  );
  const presentation = $derived(
    resolveReviewPresentation({
      layers: diff?.used_layers || Object.keys(diff?.review_policies || hostReviewPolicies),
      subkind: current?.item.subkind,
      nodeKind: current?.item.node_kind,
      policies: { ...hostReviewPolicies, ...(diff?.review_policies || {}) },
    })
  );
  const learnCurrent = $derived(
    learnEntries.length ? learnEntries[Math.min(learnCursor, learnEntries.length - 1)] : null
  );
  const sandboxProps = $derived.by(() => {
    const peek = current?.peek;
    if (!peek) return [] as string[];
    return [...(peek.fields || []), ...(peek.methods || [])].slice(0, 12);
  });
  const sandboxBody = $derived((current?.peek?.body_preview || []).slice(0, 16));
  const fileDiffsForStep = $derived.by((): FileDiff[] => {
    const all = diff?.file_diffs || [];
    if (!all.length || !current) return all;
    const name = itemDisplayName(current.item).toLowerCase();
    const path = pathOf(current.item).toLowerCase();
    const filtered = all.filter(
      (f) =>
        f.path.toLowerCase().includes(name) ||
        path.includes(f.path.toLowerCase()) ||
        f.path.toLowerCase().includes(path.split('/').pop() || '')
    );
    return filtered.length ? filtered : all.slice(0, 3);
  });

  const meta = $derived($codingSessionMeta as Record<string, unknown> | null);
  const branchName = $derived(
    (meta?.branch_name as string) || (meta?.draft_mode ? 'work' : 'main')
  );

  function rebuildGroups(from: WizardItemState[]) {
    groups = buildWizardGroups(from);
  }

  function patchItems(mutator: (it: WizardItemState) => WizardItemState) {
    items = items.map(mutator);
    rebuildGroups(items);
  }

  function impactJumpName(label: string): string {
    // Labels may be "in Foo", "→ Bar", "mentions:Baz", "Foo calls", "impl Trait"
    const t = label.trim();
    const m =
      t.match(/^(?:in|contains|impl|refs|→)\s+(.+)$/i) ||
      t.match(/^mentions:(.+)$/i) ||
      t.match(/^(.+?)\s+(?:calls|implements|refs)$/i);
    return (m?.[1] || t).trim();
  }

  function onKey(e: KeyboardEvent) {
    if (phase !== 'walk' || busy) return;
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (e.key === 'Escape') {
      e.preventDefault();
      closePrWizard();
      return;
    }
    // Learn mode: arrow keys walk the durable journal timeline.
    if (wizardMode === 'learn') {
      if (e.key === 'ArrowRight' || e.key === 'n' || e.key === 'N') {
        e.preventDefault();
        if (learnEntries.length) {
          learnCursor = Math.min(learnEntries.length - 1, learnCursor + 1);
        }
        return;
      }
      if (e.key === 'ArrowLeft' || e.key === 'p' || e.key === 'P') {
        e.preventDefault();
        learnCursor = Math.max(0, learnCursor - 1);
        return;
      }
      if (e.key === 'l' || e.key === 'L') {
        e.preventDefault();
        wizardMode = 'review';
        return;
      }
      // Block approve/reject keys in learn mode (read-only).
      if (['y', 'Y', 'a', 'A', 'f', 'F', 'r', 'R', 's', 'S'].includes(e.key)) {
        e.preventDefault();
        return;
      }
    }
    if ((e.key === 'a' || e.key === 'A') && e.shiftKey) {
      e.preventDefault();
      showNote = true;
      return;
    }
    if (e.key === 'y' || e.key === 'Y') {
      e.preventDefault();
      showNote = true;
      return;
    }
    if (e.key === 'l' || e.key === 'L') {
      e.preventDefault();
      void enterLearnMode();
      return;
    }
    if (e.key === 'a' || e.key === 'A') {
      e.preventDefault();
      if (currentGroup?.decision === 'approve') void clearDecision();
      else void decide('approve');
    } else if (e.key === 'u' || e.key === 'U') {
      e.preventDefault();
      if (currentGroup?.decision) void clearDecision();
    } else if (e.key === 'f' || e.key === 'F' || e.key === 'r' || e.key === 'R') {
      e.preventDefault();
      showFeedback = true;
    } else if (e.key === 'n' || e.key === 'N' || e.key === 'ArrowRight') {
      e.preventDefault();
      if (currentGroup?.decision) advance();
      else skip();
    } else if (e.key === 'p' || e.key === 'P' || e.key === 'ArrowLeft') {
      e.preventDefault();
      goStep(step - 1);
    } else if (e.key === 's' || e.key === 'S') {
      e.preventDefault();
      if (currentGroup?.decision === 'skip') void clearDecision();
      else skip();
    } else if (e.key === 'd' || e.key === 'D') {
      e.preventDefault();
      showFileDiff = !showFileDiff;
    } else if (e.key === 'i' || e.key === 'I') {
      e.preventDefault();
      previewDepth =
        previewDepth === 'peek' ? 'il' : previewDepth === 'il' ? 'source' : 'peek';
    } else if (e.key === 'e' || e.key === 'E') {
      e.preventDefault();
      if (currentGroup) {
        groups = groups.map((g, i) =>
          i === step ? { ...g, expanded: !g.expanded } : g
        );
      }
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

  async function enterLearnMode() {
    wizardMode = 'learn';
    learnCursor = 0;
    const name = current ? itemDisplayName(current.item) : undefined;
    const walk = await fetchLearnJournalWalk({
      construct: name,
      pr_id: prId,
      limit: 50,
    });
    learnConstructEntries = walk.construct as Record<string, unknown>[];
    learnPrEntries = walk.pr as Record<string, unknown>[];
    learnEntries = walk.merged as Record<string, unknown>[];
  }

  async function bootstrap() {
    phase = 'loading';
    error = null;
    statusMsg = null;
    prId = $prWizardChangeId;
    try {
      void fetchReviewPolicies().then((p) => {
        hostReviewPolicies = p;
      });
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
          approvedPrs = openPrs.filter((p) => p.status === 'Approved');
          otherPrs = open.filter(
            (p) => !prBelongsToProject(p, slug) || isSmokeOrFixturePr(p)
          );
        } catch {
          openPrs = [];
          approvedPrs = [];
          otherPrs = [];
        }
        // Single Approved PR for this project → land on merge pad (don't re-walk 162 steps).
        if (approvedPrs.length === 1 && openPrs.length === 1) {
          await loadPr(approvedPrs[0].id);
          return;
        }
        // No project PRs → go straight to live working-tree (normal agent-edit path).
        // Still show chooser if project PRs exist OR only smoke/other (so they can pick working tree).
        if (openPrs.length > 0 || otherPrs.length > 0 || approvedPrs.length > 0) {
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
    readyToMerge = false;
    const detail = await fetchPullRequestDetail(id);
    pr = detail.pr;
    comments = detail.comments;
    await loadDiffAndItems(id, detail.pr.description || '');
    step = 0;
    // Already approved/merged → landing pad (merge / history), not a 100-item re-walk
    if (pr.status === 'Approved' || pr.status === 'Merged') {
      readyToMerge = pr.status === 'Approved';
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
        if (refreshed.applied > 0) {
          items = refreshed.items;
          rebuildGroups(items);
        }
      } catch {
        /* keep initial items */
      }
    }
    rebuildGroups(items);
    step = 0;
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
      rebuildGroups(items);
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

  async function decide(decision: 'approve' | 'feedback', opts?: { sendNow?: boolean; note?: string }) {
    const g = currentGroup;
    if (!g || busy || wizardMode === 'learn') return;
    if (decision === 'approve' && g.decision === 'approve') {
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
      const note = (opts?.note ?? noteDraft).trim();
      const childIdx = new Set(g.children.map((c) => c.index));
      items = items.map((it) =>
        childIdx.has(it.index)
          ? {
              ...it,
              decision,
              feedback: decision === 'feedback' ? feedbackDraft.trim() : it.feedback,
              teachingNote: note || it.teachingNote,
            }
          : it
      );
      rebuildGroups(items);
      const updated = items.find((it) => childIdx.has(it.index))!;

      if (prId) {
        await postReviewItem(prId, {
          decision,
          construct_path: pathOf(updated.item),
          body:
            decision === 'feedback'
              ? updated.feedback
              : note
                ? `Approved in PR Wizard. Note: ${note}`
                : 'Approved in PR Wizard.',
          send_now: !!opts?.sendNow,
          item_index: updated.index,
          item_kind: updated.item.kind,
          item_name: itemDisplayName(updated.item),
          rationale: updated.rationale || undefined,
        });
        try {
          const d = await fetchPullRequestDetail(prId);
          comments = d.comments;
        } catch {
          /* ignore */
        }
      }

      void postJournalEntry({
        pr_id: prId,
        construct_path: pathOf(updated.item),
        construct_name: itemDisplayName(updated.item),
        decision,
        rationale: updated.rationale,
        teaching_note: note || null,
        risk: updated.criticality,
        package: currentProjectParam(),
      });

      if (decision === 'feedback' && opts?.sendNow) {
        sendFeedbackToAgent(
          [
            {
              index: updated.index,
              path: pathOf(updated.item),
              name: itemDisplayName(updated.item),
              kind: updated.item.kind,
              text: updated.feedback,
              rationale: updated.rationale,
            },
          ],
          pr?.title
        );
        items = items.map((it) =>
          childIdx.has(it.index) ? { ...it, sentToAgent: true } : it
        );
        rebuildGroups(items);
      }

      feedbackDraft = '';
      noteDraft = '';
      showFeedback = false;
      showNote = false;
      advance();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Undo approve / feedback / skip — group becomes pending again. */
  async function clearDecision() {
    const g = currentGroup;
    const cur = current;
    if (!g || !cur || busy || g.decision == null || wizardMode === 'learn') return;
    const prev = g.decision;
    busy = true;
    error = null;
    try {
      const childIdx = new Set(g.children.map((c) => c.index));
      items = items.map((it) =>
        childIdx.has(it.index)
          ? {
              ...it,
              decision: null,
              feedback: prev === 'feedback' ? '' : it.feedback,
              sentToAgent: false,
            }
          : it
      );
      rebuildGroups(items);
      feedbackDraft = '';
      showFeedback = false;
      if (prId && (prev === 'approve' || prev === 'feedback')) {
        await postReviewItem(prId, {
          decision: 'clear',
          construct_path: pathOf(cur.item),
          body: `Cleared ${prev} decision in PR Wizard.`,
          item_index: cur.index,
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
    if (wizardMode === 'learn') {
      advance();
      return;
    }
    if (currentGroup?.decision === 'skip') {
      advance();
      return;
    }
    const g = currentGroup;
    if (!g) return;
    const childIdx = new Set(g.children.map((c) => c.index));
    items = items.map((it) =>
      childIdx.has(it.index) ? { ...it, decision: 'skip' as const } : it
    );
    rebuildGroups(items);
    advance();
  }

  function advance() {
    if (step < groups.length - 1) {
      step += 1;
      const next = groups[step]?.children[0];
      feedbackDraft = next?.feedback || '';
      showFeedback = false;
      showNote = false;
    } else {
      phase = 'summary';
    }
  }

  function goStep(i: number) {
    if (i < 0 || i >= groups.length) return;
    step = i;
    phase = 'walk';
    feedbackDraft = groups[i]?.children[0]?.feedback || '';
    showFeedback = groups[i]?.decision === 'feedback';
    showNote = false;
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
      // Count from current items (avoid stale derived if walk just finished).
      const nApproved = items.filter((i) => i.decision === 'approve').length;
      const nFeedback = items.filter((i) => i.decision === 'feedback').length;
      if (!id) {
        // Create PR from session review so history is durable
        statusMsg = 'Creating PR and publishing session work…';
        const title =
          diff && (diff.added || diff.removed || diff.changed)
            ? `Review: +${diff.added} −${diff.removed} ~${diff.changed}`
            : `Review: ${branchName || 'work'}`;
        const descParts = [
          'Opened from IDE PR Wizard (session working tree).',
          '',
          '## Changes',
          ...items.slice(0, 80).map(
            (it) =>
              `- **${itemDisplayName(it.item)}** (${it.item.kind}): ${it.decision || 'pending'}${
                it.rationale ? ` — ${it.rationale.slice(0, 120)}` : ''
              }`
          ),
          items.length > 80 ? `\n…and ${items.length - 80} more.` : '',
        ];
        const created = await createAndSubmitPr({
          title,
          description: descParts.join('\n'),
          // omit main — server allocates cr/… and client publishes to it
          source_branch:
            branchName && branchName !== 'main' && branchName !== 'master'
              ? branchName
              : undefined,
        });
        id = created.id;
        prId = id;
        pr = created;
      }

      // Persist any decisions not yet posted (session-first path). Cap concurrency.
      statusMsg = `Recording ${nApproved + nFeedback} decision(s)…`;
      const pending = items.filter(
        (it) => it.decision === 'approve' || it.decision === 'feedback'
      );
      const chunk = 12;
      for (let i = 0; i < pending.length; i += chunk) {
        await Promise.all(
          pending.slice(i, i + chunk).map(async (it) => {
            try {
              await postReviewItem(id!, {
                decision: it.decision as 'approve' | 'feedback',
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
          })
        );
      }

      const summary =
        outcome === 'all_approved'
          ? `PR Wizard: approved ${nApproved} structural change(s).`
          : `PR Wizard: ${nApproved} approved, ${nFeedback} need work.\n\n` +
            queuedFeedback()
              .map((q) => `- ${q.name}: ${q.text}`)
              .join('\n');

      const fin = await finalizeWizardApi(id, {
        outcome,
        summary,
        approved_count: nApproved,
        feedback_count: nFeedback,
      });

      if (outcome === 'needs_work') {
        readyToMerge = false;
        const q = queuedFeedback();
        if (q.length) sendFeedbackToAgent(q, pr?.title);
        statusMsg = 'Changes requested — feedback sent to the agent.';
        if (pr) pr = { ...pr, status: 'ChangesRequested' };
        phase = 'done';
      } else {
        // Always show Merge after approve — do not wait on detail refresh.
        readyToMerge = true;
        statusMsg = `All ${nApproved} change(s) approved. Click Merge to land on main.`;
        if (pr) {
          pr = {
            ...pr,
            status: (fin?.status as string) || 'Approved',
          };
        } else {
          pr = {
            id,
            title: 'Approved PR',
            description: '',
            source_branch: 'work',
            target_branch: 'main',
            author: 'operator',
            status: 'Approved',
          };
        }
        phase = 'done';
      }

      try {
        const d = await fetchPullRequestDetail(id);
        pr = d.pr;
        comments = d.comments;
        if (pr.status === 'Approved') readyToMerge = true;
      } catch {
        /* keep optimistic status */
      }
    } catch (e) {
      error = String(e);
      statusMsg = null;
    } finally {
      busy = false;
    }
  }

  async function doMerge() {
    if (!prId) return;
    busy = true;
    error = null;
    statusMsg = 'Merging to main…';
    try {
      const result = await mergeChangeApi(prId, currentProjectParam());
      readyToMerge = false;
      statusMsg =
        'Merged to main. Product base updated. Re-open Review to confirm the working tree is clear.';
      if (pr) pr = { ...pr, status: 'Merged' };
      // Nudge session endpoint so clients refresh meta after land.
      const sid = getCodingSessionId();
      if (sid) {
        try {
          await fetch(`${platformRoot()}/api/sessions/${sid}`, {
            headers: ideRequestHeaders(),
          });
        } catch {
          /* ignore */
        }
      }
      void result;
    } catch (e) {
      error = String(e);
      statusMsg = null;
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

  {#if phase === 'walk' && groups.length > 0}
    <div class="progress" aria-hidden="true">
      <div class="bar" style="width: {Math.max(4, ((step + 1) / groups.length) * 100)}%"></div>
    </div>
    <div class="step-meta">
      <span
        >Step {step + 1} of {groups.length}
        {#if groups[step]?.children.length > 1}
          · {groups[step].children.length} changes
        {/if}
      </span>
      <span class="counts">
        <span class="ok">{approvedCount} ✓</span>
        <span class="fb">{feedbackCount} 💬</span>
        <span class="pend">{pendingCount} left</span>
      </span>
      <span class="keys dim" title="Keyboard"
        >A approve · Y note · R feedback · S skip · D file diff · I depth · E expand · N/P · Esc</span
      >
      {#if wizardMode === 'learn'}
        <span class="pill sm">Learn mode</span>
      {/if}
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

      {#if approvedPrs.length > 0}
        <section class="pick-section approved-pad">
          <h4>Ready to merge ({approvedPrs.length})</h4>
          <p class="muted sm">
            You already finished a structural walk. Do <strong>not</strong> re-walk the working
            tree — merge the approved PR to land on main.
          </p>
          <ul class="pr-list">
            {#each approvedPrs as p}
              <li>
                <button type="button" class="pr-row" onclick={() => void loadPr(p.id)}>
                  <div class="pr-row-main">
                    <span class="pill sm">Approved</span>
                    <span class="t">{p.title}</span>
                  </div>
                  <div class="pr-row-meta">
                    <code>{p.source_branch}</code>
                    <span class="meta-arrow">→</span>
                    <code>{p.target_branch || 'main'}</code>
                  </div>
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      <!-- Live edits in this IDE session -->
      <section class="pick-section">
        <h4>This IDE session</h4>
        <p class="muted sm">
          Uncommitted or recently edited work on branch
          <code>{branchName}</code>
          {#if currentProjectParam()}
            in project <code>{currentProjectParam()}</code>
          {/if}.
          Use this after the agent just finished editing — even if no PR exists yet.
          {#if approvedPrs.length > 0}
            <br />
            <strong class="warn-inline"
              >An approved PR is waiting — re-reviewing the working tree will show the same
              steps until you merge.</strong
            >
          {/if}
        </p>
        <button
          type="button"
          class="btn pick-primary"
          class:primary={approvedPrs.length === 0}
          onclick={() => void loadSessionReview()}
        >
          Review current working tree
        </button>
      </section>

      <!-- Formal pull requests for THIS project -->
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
    {:else if phase === 'walk' && current && currentGroup}
      {@const it = current.item}
      {@const g = currentGroup}
      <div class="item-card" class:add={itemKindClass(it.kind) === 'add'} class:rem={itemKindClass(it.kind) === 'rem'} class:chg={itemKindClass(it.kind) === 'chg'}>
        <div class="item-top">
          <span class="kind-badge {itemKindClass(it.kind)}">{itemKindLabel(it.kind)}</span>
          <h3 class="item-name">{g.name}</h3>
          <span class="risk-chip risk-{g.risk}" title="Review risk">{riskLabel(g.risk)}</span>
          {#if it.subkind}
            <span class="pill sm">{it.subkind}</span>
          {/if}
          {#if it.node_kind}
            <span class="dim">{it.node_kind}</span>
          {/if}
          {#if g.children.length > 1}
            <span class="pill sm">{g.children.length} parts</span>
          {/if}
        </div>
        <p class="container">{containerLabel(it)}</p>
        <p class="path"><code>{pathOf(it)}</code></p>

        <!-- Evidence strip -->
        <div class="evidence-strip">
          <span class:on={!!current.rationale}>Rationale {current.rationale ? '✓' : '—'}</span>
          <span class:on={g.risk === 'critical' || g.risk === 'high'}>Risk {riskLabel(g.risk)}</span>
          <span class:on={(g.impact?.length || 0) > 0}
            >Impact {g.impact?.length || 0}</span
          >
          <span class:on={presentation.strategy === 'component_sandbox'}
            >Preview {presentation.strategy}</span
          >
          <span class:on={previewDepth !== 'peek'}>View {previewDepth}</span>
        </div>

        {#if g.impact?.length}
          <section class="impact-section">
            <h4>Blast radius</h4>
            <ul class="impact-list">
              {#each g.impact as name}
                <li>
                  <button
                    type="button"
                    class="linkish"
                    onclick={() => focusConstructByName(impactJumpName(name))}
                    >{name}</button
                  >
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        {#if current.rationale}
          <section class="rationale">
            <h4>Agent rationale</h4>
            <p>{current.rationale}</p>
          </section>
        {:else if wizardMode === 'review'}
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

        {#if g.expanded && g.children.length > 1}
          <section class="group-parts">
            <h4>Grouped changes</h4>
            <ul>
              {#each g.children as ch}
                <li>
                  <span class="kind-badge sm {itemKindClass(ch.item.kind)}"
                    >{itemKindLabel(ch.item.kind)}</span
                  >
                  {itemDisplayName(ch.item)}
                  {#if ch.decision}<span class="dim">· {ch.decision}</span>{/if}
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        <!-- Construct snapshot / IL -->
        {#if previewDepth !== 'source' && (current.peek || current.peekBase)}
          <section class="peek-section">
            <h4>
              {previewDepth === 'il' ? 'IL detail' : 'Construct'}
              <button type="button" class="ghost sm" onclick={() => (previewDepth = previewDepth === 'il' ? 'peek' : 'il')}
                >{previewDepth === 'il' ? 'Simpler' : 'Deeper (I)'}</button
              >
            </h4>
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
            {#if previewDepth === 'il' && current.peek}
              <div class="il-extra">
                {#if current.peek.signature}
                  <p><strong>Signature</strong> <code>{current.peek.signature}</code></p>
                {/if}
                {#if current.peek.fields?.length}
                  <p><strong>Fields</strong></p>
                  <ul>{#each current.peek.fields as f}<li><code>{f}</code></li>{/each}</ul>
                {/if}
                {#if current.peek.methods?.length}
                  <p><strong>Methods</strong></p>
                  <ul>{#each current.peek.methods as m}<li><code>{m}</code></li>{/each}</ul>
                {/if}
                {#if current.peek.body_preview?.length}
                  <p><strong>Body</strong></p>
                  <pre class="body-il">{current.peek.body_preview.join('\n')}</pre>
                {/if}
              </div>
            {/if}
          </section>
        {:else if beforeLines(it).length || afterLines(it).length || it.kind === 'signature_changed' || previewDepth === 'source'}
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

        <!-- Secondary file diff (key D) -->
        <section class="file-diff-section">
          <button
            type="button"
            class="ghost sm"
            onclick={() => (showFileDiff = !showFileDiff)}
          >
            {showFileDiff ? 'Hide' : 'Show'} file diff (D)
            {#if fileDiffsForStep.length}
              · {fileDiffsForStep.length} file(s)
            {/if}
          </button>
          {#if showFileDiff}
            {#if fileDiffsForStep.length === 0}
              <p class="muted tiny">No file-level hunks for this step.</p>
            {:else}
              {#each fileDiffsForStep as fd}
                <div class="file-diff-block">
                  <div class="file-diff-h">
                    <code>{fd.path}</code>
                    <span class="pill sm">{fd.status}</span>
                  </div>
                  {#each fd.hunks || [] as hunk}
                    <pre class="file-diff-pre"
                      >{hunk.header || ''}
{#each hunk.lines || [] as line}{line}
{/each}</pre
                    >
                  {/each}
                </div>
              {/each}
            {/if}
          {/if}
        </section>

        {#if presentation.strategy === 'component_sandbox'}
          <section class="sandbox-frame">
            <div class="sandbox-chrome">
              <span class="pill sm">component_sandbox</span>
              {#if presentation.target}<code>{presentation.target}</code>{/if}
              {#if presentation.fromLayer}
                <span class="dim">layer:{presentation.fromLayer}</span>
              {/if}
              <span class="dim">fallback:{presentation.fallback}</span>
            </div>
            <div class="sandbox-stage" aria-label="Component surface preview">
              <header class="sandbox-title">
                <strong>{itemDisplayName(it)}</strong>
                {#if current?.peek?.subkind}
                  <span class="pill sm">{current.peek.subkind}</span>
                {/if}
              </header>
              {#if sandboxProps.length}
                <div class="sandbox-props">
                  <span class="dim">props / surface</span>
                  <ul>
                    {#each sandboxProps as p}
                      <li><code>{p}</code></li>
                    {/each}
                  </ul>
                </div>
              {/if}
              {#if sandboxBody.length}
                <pre class="sandbox-body">{sandboxBody.join('\n')}</pre>
              {:else if current?.peek?.signature}
                <pre class="sandbox-body">{current.peek.signature}</pre>
              {:else}
                <p class="muted tiny">
                  No body surface yet — structural peek above is the live fallback.
                </p>
              {/if}
            </div>
            <p class="muted tiny">
              Isolated Vite iframe lands with package UI sandbox; this panel is the layer-declared
              surface preview (props + body) so review never depends on a running app.
            </p>
          </section>
        {/if}

        <div class="item-actions-row">
          <button type="button" class="btn" onclick={() => jumpConstruct(it)}>
            Show in IDE
          </button>
          {#if wizardMode === 'review'}
            <button type="button" class="ghost" onclick={() => void enterLearnMode()}>
              Learn mode
            </button>
          {:else}
            <button type="button" class="ghost" onclick={() => (wizardMode = 'review')}>
              Review mode
            </button>
          {/if}
        </div>
        {#if wizardMode === 'learn'}
          <section class="learn-walk">
            <h4>Design journal walk</h4>
            <p class="muted tiny">
              Read-only. Construct-scoped → this PR → global history. Use ← → to step journal
              entries (review keys Y/D still apply to wizard steps).
            </p>
            <div class="learn-meta">
              <span class="pill sm">{learnConstructEntries.length} construct</span>
              <span class="pill sm">{learnPrEntries.length} PR</span>
              <span class="pill sm">{learnEntries.length} total</span>
              {#if learnEntries.length}
                <span class="dim">{learnCursor + 1}/{learnEntries.length}</span>
              {/if}
            </div>
            {#if learnCurrent}
              <article class="learn-card">
                <div class="eh">
                  <strong>{learnCurrent.construct_name || '—'}</strong>
                  <span class="pill sm">{learnCurrent.decision}</span>
                  {#if learnCurrent.risk}<span class="dim">{learnCurrent.risk}</span>{/if}
                  <span class="dim">{String(learnCurrent.ts || '').slice(0, 19)}</span>
                </div>
                {#if learnCurrent.rationale}
                  <p class="body">{learnCurrent.rationale}</p>
                {/if}
                {#if learnCurrent.teaching_note}
                  <p class="note"><strong>Teaching:</strong> {learnCurrent.teaching_note}</p>
                {/if}
                <code class="path">{learnCurrent.construct_path}</code>
              </article>
              <div class="fb-actions">
                <button
                  type="button"
                  class="ghost"
                  disabled={learnCursor <= 0}
                  onclick={() => (learnCursor = Math.max(0, learnCursor - 1))}
                >
                  ← Prior decision
                </button>
                <button
                  type="button"
                  class="ghost"
                  disabled={learnCursor >= learnEntries.length - 1}
                  onclick={() =>
                    (learnCursor = Math.min(learnEntries.length - 1, learnCursor + 1))
                  }
                >
                  Next decision →
                </button>
              </div>
            {:else}
              <p class="muted">
                No journal entries yet. Approve steps with notes (Y) to grow the durable design
                record.
              </p>
            {/if}
          </section>
        {/if}
        {#if jumpMsg}
          <p class="jump-msg">{jumpMsg}</p>
        {/if}

        {#if showNote && wizardMode === 'review'}
          <div class="feedback-box">
            <label for="note">Accept with note (optional teaching note)</label>
            <textarea
              id="note"
              rows="2"
              placeholder="Why this is correct / what future readers should know…"
              bind:value={noteDraft}
            ></textarea>
            <div class="fb-actions">
              <button
                type="button"
                class="btn primary"
                disabled={busy}
                onclick={() => void decide('approve', { note: noteDraft })}
              >
                Approve with note ✓
              </button>
              <button type="button" class="ghost" onclick={() => (showNote = false)}>Cancel</button>
            </div>
          </div>
        {/if}

        {#if showFeedback && wizardMode === 'review'}
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

      <!-- step strip = groups -->
      <div class="step-strip" role="tablist">
        {#each groups as g, i}
          <button
            type="button"
            class="dot risk-{g.risk}"
            class:current={i === step}
            class:ok={g.decision === 'approve'}
            class:fb={g.decision === 'feedback'}
            class:skip={g.decision === 'skip'}
            title="{g.name} — {riskLabel(g.risk)} — {g.decision || 'pending'}"
            onclick={() => goStep(i)}
          ></button>
        {/each}
      </div>
    {:else if phase === 'summary' || (phase === 'walk' && items.length === 0)}
      <div class="summary">
        <h3>Review summary</h3>
        {#if items.length === 0}
          <div class="empty-diff-banner">
            <p class="muted">
              <strong>Nothing to review</strong> — no structural construct changes vs baseline
              {#if diffSource === 'working-tree'}in the working tree{:else}on this PR branch{/if}.
            </p>
            {#if diffNote}
              <p class="muted sm">{diffNote}</p>
            {/if}
            {#if diff?.base_label}
              <p class="muted sm">Baseline: <code>{diff.base_label}</code></p>
            {/if}
            {#if diff?.uncommitted === false}
              <p class="muted sm">
                Session is clean (agent agrees — no outstanding uncommitted writes).
              </p>
            {/if}
            {#if diff?.description}
              <p class="muted sm">{diff.description}</p>
            {/if}
            {#if diff?.file_diffs?.length}
              <p class="muted sm">
                {diff.file_diffs.length} file-level change(s) exist (key D on a step when present).
                Structural review needs .veil / .layer edits that change constructs.
              </p>
            {:else}
              <p class="muted sm">
                Submit is gated against empty diffs — publish session work to the PR branch, or
                edit constructs first. Force-submit only with <code>?force=1</code>.
              </p>
            {/if}
            {#if diff?.parse_notes?.length}
              <details class="muted sm">
                <summary>Parse notes</summary>
                <ul>
                  {#each diff.parse_notes as n}
                    <li>{n}</li>
                  {/each}
                </ul>
              </details>
            {/if}
          </div>
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
        <h3>
          {pr?.status === 'Merged'
            ? 'Merged'
            : pr?.status === 'Approved' || readyToMerge
              ? 'Approved — merge to land'
              : 'Done'}
        </h3>
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
            Nothing is on product main until you click Merge.
          </p>
          {#if (pr.status === 'Approved' || readyToMerge) && prId}
            <div class="merge-cta">
              <button
                type="button"
                class="btn primary merge-big"
                disabled={busy}
                onclick={() => void doMerge()}
              >
                {busy ? 'Merging…' : `Merge to ${pr.target_branch || 'main'}`}
              </button>
              <p class="muted sm">
                This promotes the published feature branch into main and closes the PR. Re-opening
                “Review working tree” before merge will still show the same steps.
              </p>
            </div>
          {/if}
          {#if diffSource === 'pr-empty'}
            <p class="banner-warn">
              No structural snapshot on this PR’s branch — there is nothing to re-approve item-by-item.
            </p>
          {:else if items.length > 0 && pr.status !== 'Approved' && !readyToMerge}
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
    {#if phase === 'walk' && currentGroup}
      <button type="button" class="ghost" disabled={step === 0} onclick={() => goStep(step - 1)}>
        ← Back
      </button>
      <div class="spacer"></div>
      {#if wizardMode === 'learn'}
        <button type="button" class="btn" onclick={() => advance()}>
          {step < groups.length - 1 ? 'Next →' : 'Done →'}
        </button>
      {:else if currentGroup.decision}
        <span class="decision-badge" class:ok={currentGroup.decision === 'approve'} class:fb={currentGroup.decision === 'feedback'} class:skip={currentGroup.decision === 'skip'}>
          {#if currentGroup.decision === 'approve'}Approved
          {:else if currentGroup.decision === 'feedback'}Feedback queued
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
          {step < groups.length - 1 ? 'Next →' : 'Summary →'}
        </button>
        {#if currentGroup.decision !== 'approve'}
          <button
            type="button"
            class="btn primary"
            disabled={busy}
            onclick={() => void decide('approve')}
          >
            Approve ✓
          </button>
        {/if}
        {#if currentGroup.decision !== 'feedback'}
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
          class="ghost"
          disabled={busy}
          title="Approve with note (Y)"
          onclick={() => (showNote = true)}
        >
          Note…
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
      {#if items.length > 0 && diffSource !== 'pr-empty' && pr?.status !== 'Approved' && !readyToMerge}
        <button type="button" class="btn" onclick={() => startWalkFromDone()}>
          Browse {items.length} structural items
        </button>
      {/if}
      {#if (pr?.status === 'Approved' || readyToMerge) && prId}
        <button type="button" class="btn primary" disabled={busy} onclick={() => void doMerge()}>
          {busy ? 'Merging…' : `Merge to ${pr?.target_branch || 'main'}`}
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
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .risk-chip {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.15rem 0.45rem;
    border-radius: 999px;
    border: 1px solid rgba(115, 115, 115, 0.35);
  }
  .risk-critical {
    color: #fca5a5;
    border-color: rgba(239, 68, 68, 0.5);
    background: rgba(239, 68, 68, 0.12);
  }
  .risk-high {
    color: #fdba74;
    border-color: rgba(249, 115, 22, 0.45);
    background: rgba(249, 115, 22, 0.1);
  }
  .risk-normal {
    color: #93c5fd;
    border-color: rgba(59, 130, 246, 0.4);
    background: rgba(59, 130, 246, 0.08);
  }
  .risk-low {
    color: #a3a3a3;
    border-color: rgba(115, 115, 115, 0.35);
  }
  .evidence-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 0.45rem;
    margin: 0.5rem 0 0.65rem;
    font-size: 0.68rem;
    color: #737373;
  }
  .evidence-strip span {
    padding: 0.15rem 0.4rem;
    border-radius: 6px;
    border: 1px solid rgba(115, 115, 115, 0.25);
    background: rgba(0, 0, 0, 0.2);
  }
  .evidence-strip span.on {
    color: #a7f3d0;
    border-color: rgba(52, 211, 153, 0.35);
  }
  .impact-section h4,
  .group-parts h4,
  .file-diff-section h4 {
    margin: 0.65rem 0 0.35rem;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: #a3a3a3;
  }
  .impact-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .impact-list li {
    margin: 0;
  }
  .linkish {
    background: none;
    border: none;
    color: #93c5fd;
    cursor: pointer;
    font-size: 0.8rem;
    padding: 0;
    text-decoration: underline;
  }
  .group-parts ul {
    list-style: none;
    padding: 0;
    margin: 0;
    font-size: 0.8rem;
  }
  .group-parts li {
    padding: 0.25rem 0;
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .file-diff-block {
    margin-top: 0.45rem;
    border: 1px solid rgba(115, 115, 115, 0.3);
    border-radius: 8px;
    overflow: hidden;
  }
  .file-diff-h {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    padding: 0.35rem 0.55rem;
    background: rgba(0, 0, 0, 0.35);
    font-size: 0.75rem;
  }
  .file-diff-pre {
    margin: 0;
    padding: 0.5rem 0.65rem;
    font-size: 0.7rem;
    line-height: 1.35;
    overflow-x: auto;
    max-height: 220px;
    background: #0a0a0a;
    color: #d4d4d4;
    white-space: pre-wrap;
  }
  .il-extra {
    margin-top: 0.5rem;
    font-size: 0.78rem;
  }
  .il-extra ul {
    margin: 0.2rem 0 0.5rem;
    padding-left: 1.1rem;
  }
  .body-il {
    margin: 0.25rem 0 0;
    padding: 0.5rem;
    font-size: 0.72rem;
    background: rgba(0, 0, 0, 0.35);
    border-radius: 6px;
    max-height: 180px;
    overflow: auto;
  }
  .sandbox-placeholder,
  .sandbox-frame {
    margin-top: 0.65rem;
    padding: 0.55rem 0.7rem;
    border: 1px dashed rgba(167, 139, 250, 0.4);
    border-radius: 8px;
    background: rgba(139, 92, 246, 0.06);
  }
  .sandbox-chrome {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
    margin-bottom: 0.45rem;
    font-size: 0.72rem;
  }
  .sandbox-stage {
    border: 1px solid rgba(167, 139, 250, 0.35);
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.35);
    padding: 0.65rem 0.75rem;
    min-height: 4rem;
  }
  .sandbox-title {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    margin-bottom: 0.4rem;
  }
  .sandbox-props ul {
    margin: 0.2rem 0 0.4rem;
    padding-left: 1.1rem;
    font-size: 0.78rem;
  }
  .sandbox-body {
    margin: 0.25rem 0 0;
    padding: 0.45rem 0.55rem;
    font-size: 0.72rem;
    background: #0a0a0a;
    border-radius: 6px;
    max-height: 160px;
    overflow: auto;
    white-space: pre-wrap;
    color: #d4d4d4;
  }
  .learn-walk {
    margin-top: 0.75rem;
    padding: 0.65rem 0.75rem;
    border: 1px solid rgba(59, 130, 246, 0.35);
    border-radius: 8px;
    background: rgba(59, 130, 246, 0.06);
  }
  .learn-walk h4 {
    margin: 0 0 0.35rem;
    font-size: 0.85rem;
  }
  .learn-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    align-items: center;
    margin: 0.35rem 0 0.5rem;
  }
  .learn-card {
    padding: 0.55rem 0.65rem;
    border-radius: 8px;
    border: 1px solid rgba(115, 115, 115, 0.3);
    background: rgba(0, 0, 0, 0.25);
  }
  .learn-card .eh {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }
  .learn-card .body {
    margin: 0.35rem 0 0;
    font-size: 0.82rem;
  }
  .learn-card .note {
    margin: 0.25rem 0 0;
    color: #c4b5fd;
    font-size: 0.8rem;
  }
  .learn-card .path {
    display: block;
    margin-top: 0.35rem;
    font-size: 0.7rem;
    color: #737373;
  }
  .empty-diff-banner {
    padding: 0.75rem 0.85rem;
    border-radius: 8px;
    border: 1px solid rgba(251, 191, 36, 0.35);
    background: rgba(251, 191, 36, 0.08);
  }
  .pick-section.approved-pad {
    border-color: rgba(34, 197, 94, 0.45);
    background: rgba(34, 197, 94, 0.08);
  }
  .warn-inline {
    color: #fbbf24;
    font-weight: 600;
  }
  .merge-cta {
    margin: 1rem 0;
    padding: 0.85rem 1rem;
    border-radius: 10px;
    border: 1px solid rgba(34, 197, 94, 0.45);
    background: rgba(34, 197, 94, 0.1);
  }
  .merge-big {
    font-size: 1rem;
    padding: 0.65rem 1.25rem;
    width: 100%;
  }
  .btn.sm,
  .ghost.sm {
    font-size: 0.72rem;
    padding: 0.2rem 0.5rem;
  }
  .dot.risk-critical {
    box-shadow: 0 0 0 1px rgba(239, 68, 68, 0.55);
  }
  .dot.risk-high {
    box-shadow: 0 0 0 1px rgba(249, 115, 22, 0.45);
  }
</style>
