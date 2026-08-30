/**
 * Review PR API — network + agent-handoff functions for the /review ceremony.
 *
 * Loads structural diffs, lists/fetches pull requests, posts review items and
 * journal entries, creates/submits/merges PRs, and hands feedback to the agent.
 * Pure grouping/rationale logic lives in `grouping.ts`; shared types in `types.ts`.
 */
import { get } from 'svelte/store';
import {
  apiHost,
  currentProjectParam,
  getCodingSessionId,
  ideApiBase,
  ideRequestHeaders,
  codingSessionMeta,
} from '$lib/ide/store';
import { agentSend, openAgentPanel } from '$lib/agent/runtimeAgentSession';
import { mergeRationalesIntoItems, rationalesFromPrTexts } from './grouping';
import type {
  LayerReviewPolicy,
  PullRequest,
  QueuedFeedback,
  ReviewComment,
  StructDiff,
  WizardItemState,
} from './types';

/** Does this pull request belong to the open IDE project? */
export function prBelongsToProject(pr: PullRequest, slug: string | null): boolean {
  if (!slug) return true;
  const s = slug.toLowerCase();
  const desc = (pr.description || '').toLowerCase();
  const title = (pr.title || '').toLowerCase();
  const branch = (pr.source_branch || '').toLowerCase();
  if (desc.includes(`project: ${s}`)) return true;
  if (desc.includes(`slug: ${s}`)) return true;
  if (title.includes(s)) return true;
  // Agent often uses feature branches named after work, not the project — weak signal
  if (branch === s) return true;
  return false;
}

/** API smoke / fixture PRs from platform bring-up — not real product work. */
export function isSmokeOrFixturePr(pr: PullRequest): boolean {
  const t = (pr.title || '').toLowerCase();
  return (
    t.includes('smoke') ||
    t.includes('100pct') ||
    t.includes('pr wizard smoke') ||
    t.startsWith('test ') ||
    t.includes('fixture')
  );
}

export function platformRoot(): string {
  return apiHost() || (typeof window !== 'undefined' ? window.location.origin : '');
}

/**
 * Live refresh: re-fetch structural diff (annotations from write_source.rationales)
 * + re-parse PR texts. Preserves approve/feedback decisions on each step.
 *
 * Always consults the **working-tree** `/diff` for the in-process rationale cache
 * (write_source → record_rationales). PR branch diffs do not carry that cache.
 */
export async function refreshWizardRationales(opts: {
  prId: string | null;
  slug?: string | null;
  description?: string;
  comments?: ReviewComment[] | null;
  items: WizardItemState[];
}): Promise<{ items: WizardItemState[]; diff: StructDiff | null; applied: number }> {
  const textRats = rationalesFromPrTexts(opts.description || '', opts.comments);
  let nextDiff: StructDiff | null = null;
  try {
    // Live intent cache is always on the project working-tree endpoint.
    const wt = await loadWizardDiff({
      prId: null,
      slug: opts.slug ?? null,
      allowWorkingTreeFallback: true,
    });
    nextDiff = wt.diff;
  } catch {
    nextDiff = null;
  }

  const before = opts.items.filter((i) => i.rationale).length;
  const merged = mergeRationalesIntoItems(opts.items, textRats, nextDiff);
  const after = merged.filter((i) => i.rationale).length;
  return { items: merged, diff: nextDiff, applied: Math.max(0, after - before) };
}

export async function fetchOpenPullRequests(status?: string): Promise<PullRequest[]> {
  const u = new URL(`${platformRoot()}/api/pull_requests`);
  if (status) u.searchParams.set('status', status);
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), 12000);
  try {
    const r = await fetch(u.toString(), { headers: ideRequestHeaders(), signal: ctrl.signal });
    if (!r.ok) throw new Error(`list changes HTTP ${r.status}`);
    const data = await r.json();
    return Array.isArray(data) ? data : data.pull_requests || data.items || [];
  } finally {
    clearTimeout(t);
  }
}

export async function fetchPullRequestDetail(id: string): Promise<{
  pr: PullRequest;
  comments: ReviewComment[];
  approvals: unknown[];
  ci_runs: unknown[];
}> {
  const r = await fetch(`${platformRoot()}/api/pull_requests/${id}`, {
    headers: ideRequestHeaders(),
  });
  if (!r.ok) throw new Error(`get change HTTP ${r.status}`);
  const d = await r.json();
  const pr = (d.pr && typeof d.pr === 'object' ? d.pr : d) as PullRequest;
  // Bind PR for agent reply writeback while reviewing.
  const sid = getCodingSessionId();
  if (sid && pr.id) {
    void fetch(`${platformRoot()}/api/sessions/${sid}/active-change`, {
      method: 'POST',
      headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
      body: JSON.stringify({ pr_id: pr.id }),
    }).catch(() => {});
  }
  return {
    pr,
    comments: Array.isArray(d.comments) ? d.comments : [],
    approvals: Array.isArray(d.approvals) ? d.approvals : [],
    ci_runs: Array.isArray(d.ci_runs) ? d.ci_runs : [],
  };
}

/**
 * Load structural diff for review.
 * - Working-tree mode (no prId): IDE `/diff` (live session vs baseline).
 * - PR mode: **only** the CR branch diff. Do **not** silently substitute the live
 *   working tree (that produced "124 changes" on an Approved smoke PR that had
 *   nothing to do with the open project).
 */
export async function loadWizardDiff(opts: {
  prId: string | null;
  slug: string | null;
  /** When true, allow working-tree fallback (session review only). */
  allowWorkingTreeFallback?: boolean;
}): Promise<{
  diff: StructDiff;
  source: 'pr' | 'working-tree' | 'pr-empty';
  note?: string;
}> {
  const slug = opts.slug || currentProjectParam();
  if (opts.prId) {
    try {
      const u = new URL(`${platformRoot()}/api/pull_requests/${opts.prId}/diff`);
      if (slug) u.searchParams.set('slug', slug);
      const r = await fetch(u.toString(), { headers: ideRequestHeaders() });
      if (r.ok) {
        const diff = (await r.json()) as StructDiff;
        if (!Array.isArray(diff.items)) diff.items = [];
        if (!Array.isArray(diff.file_diffs)) diff.file_diffs = [];
        if (diff.items.length > 0 || diff.file_diffs.length > 0) {
          return { diff, source: 'pr' };
        }
        return {
          diff,
          source: 'pr-empty',
          note:
            'This PR has no file or construct changes on its branch (often smoke/test PRs or never published). ' +
            'It is not the same as your live Agent Registry edits. Use “Review current working tree” for those.',
        };
      }
    } catch (e) {
      return {
        diff: emptyDiff(),
        source: 'pr-empty',
        note: `Could not load PR diff: ${e}`,
      };
    }
    return {
      diff: emptyDiff(),
      source: 'pr-empty',
      note: 'PR diff unavailable.',
    };
  }
  const wtBase = slug
    ? `${platformRoot()}/api/p/${encodeURIComponent(slug)}`
    : ideApiBase();
  const r = await fetch(`${wtBase}/diff`, { headers: ideRequestHeaders() });
  if (!r.ok) throw new Error(`working-tree diff HTTP ${r.status}`);
  const diff = (await r.json()) as StructDiff;
  if (!Array.isArray(diff.items)) diff.items = [];

  // Guard: missing baseline used to mark every construct as Added while the
  // session was clean (agent: no outstanding changes). Refuse that phantom walk.
  const phantom =
    diff.phantom_full_add === true ||
    (diff.uncommitted === false &&
      (diff.removed || 0) === 0 &&
      (diff.changed || 0) === 0 &&
      (diff.added || 0) > 0 &&
      (!diff.base_label ||
        diff.base_label.includes('no baseline') ||
        diff.base_label.includes('(no baseline)')));
  if (phantom) {
    return {
      diff: {
        ...diff,
        items: [],
        added: 0,
        removed: 0,
        changed: 0,
        phantom_full_add: true,
      },
      source: 'working-tree',
      note:
        'Working tree is clean (matches the agent). Structural review was empty because ' +
        'there is no uncommitted delta vs the session baseline — not 100+ new constructs. ' +
        'If you expected real edits, write/commit them first, or open a feature branch PR.',
    };
  }
  return { diff, source: 'working-tree' };
}

function emptyDiff(): StructDiff {
  return {
    base_label: '',
    head_label: '',
    items: [],
    added: 0,
    removed: 0,
    changed: 0,
  };
}

export async function postReviewItem(
  prId: string,
  body: {
    /** approve | feedback | clear (undo prior decision for this construct) */
    decision: 'approve' | 'feedback' | 'clear';
    construct_path?: string;
    body?: string;
    send_now?: boolean;
    item_index?: number;
    item_kind?: string;
    item_name?: string;
    rationale?: string;
  }
): Promise<void> {
  const r = await fetch(`${platformRoot()}/api/pull_requests/${prId}/review-item`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`review-item HTTP ${r.status}: ${await r.text()}`);
}

export async function finalizeWizardApi(
  prId: string,
  body: {
    outcome: 'all_approved' | 'needs_work';
    summary?: string;
    approved_count?: number;
    feedback_count?: number;
  }
): Promise<{ status?: string }> {
  const r = await fetch(`${platformRoot()}/api/pull_requests/${prId}/finalize-wizard`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`finalize HTTP ${r.status}: ${await r.text()}`);
  return r.json();
}

export async function mergeChangeApi(
  prId: string,
  slug?: string | null
): Promise<Record<string, unknown>> {
  const r = await fetch(`${platformRoot()}/api/pull_requests/${prId}/merge`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({
      merger: 'operator',
      slug: slug || currentProjectParam() || '',
    }),
  });
  const text = await r.text();
  let data: Record<string, unknown> = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (!r.ok) {
    throw new Error(
      `merge HTTP ${r.status}: ${(data.message as string) || (data.error as string) || text}`
    );
  }
  if (data.ok === false) {
    throw new Error(String(data.message || data.error || 'merge rejected'));
  }
  return data;
}

export async function createAndSubmitPr(opts: {
  title: string;
  description: string;
  source_branch?: string;
  slug?: string;
}): Promise<PullRequest> {
  const slug = opts.slug || currentProjectParam() || '';
  const meta = get(codingSessionMeta) as Record<string, unknown> | null;
  const sessionBranch =
    opts.source_branch ||
    (meta?.branch_name as string) ||
    (meta?.draft_mode ? 'work' : undefined);
  // Never pin a PR to main — server allocates `cr/{jira}/{title}` when omitted /
  // when session is on mainline. Passing main overwrote that branch and made merge
  // a no-op (main → main) while skipping publish.
  const sourceBranchForCreate =
    sessionBranch &&
    sessionBranch.toLowerCase() !== 'main' &&
    sessionBranch.toLowerCase() !== 'master'
      ? sessionBranch
      : undefined;

  // Attach recent session commits for history/rationales
  let desc = opts.description;
  const sid = getCodingSessionId();
  if (sid) {
    try {
      const cr = await fetch(`${platformRoot()}/api/sessions/${sid}/commits`, {
        headers: ideRequestHeaders(),
      });
      if (cr.ok) {
        const data = await cr.json();
        const commits = (data.commits || []) as { message?: string; commit_id?: string }[];
        if (commits.length) {
          desc +=
            '\n\n## Commits\n' +
            commits
              .slice(0, 20)
              .map((c) => `- ${c.message || c.commit_id?.slice(0, 8) || 'commit'}`)
              .join('\n');
        }
      }
    } catch {
      /* ignore */
    }
  }
  if (slug) {
    desc = `project: ${slug}\n\n` + desc;
  }

  const createBody: Record<string, unknown> = {
    title: opts.title,
    description: desc,
    slug,
    author: 'operator',
    jira_ticket: `VEIL-${Date.now().toString(36).toUpperCase()}`,
  };
  if (sourceBranchForCreate) {
    createBody.source_branch = sourceBranchForCreate;
  }

  const create = await fetch(`${platformRoot()}/api/pull_requests`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify(createBody),
  });
  if (!create.ok) throw new Error(`create PR HTTP ${create.status}: ${await create.text()}`);
  const created = await create.json();
  const pr = (created.pull_request || created) as PullRequest;
  if (!pr.id) throw new Error('create PR returned no id');

  // Always publish session worktree onto the PR's feature branch so merge has files.
  const pubBranch = pr.source_branch;
  if (sid && pubBranch && pubBranch.toLowerCase() !== 'main' && pubBranch.toLowerCase() !== 'master') {
    try {
      const pub = await fetch(`${platformRoot()}/api/sessions/${sid}/publish-branch`, {
        method: 'POST',
        headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
        body: JSON.stringify({ branch_name: pubBranch, pr_id: pr.id }),
      });
      if (!pub.ok) {
        console.warn('publish-branch failed', await pub.text());
      }
    } catch (e) {
      console.warn('publish-branch failed', e);
    }
  } else if (sid) {
    try {
      await fetch(`${platformRoot()}/api/sessions/${sid}/active-change`, {
        method: 'POST',
        headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
        body: JSON.stringify({ pr_id: pr.id }),
      });
    } catch {
      /* ignore */
    }
  }

  const sub = await fetch(`${platformRoot()}/api/pull_requests/${pr.id}/submit?force=1`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({}),
  });
  if (!sub.ok) {
    // Draft still usable
    console.warn('submit failed', await sub.text());
  }
  return pr;
}

export function formatFeedbackPrompt(queue: QueuedFeedback[], prTitle?: string): string {
  const project = currentProjectParam() || 'this project';
  const lines = [
    prTitle
      ? `Address Review feedback on \`${prTitle}\` for project \`${project}\`.`
      : `Address Review feedback for project \`${project}\`.`,
    '',
    'Fix each item. After edits: veil_check (fix new diags same turn) → session_commit.',
    'When done: update the change description if needed — do NOT merge unless asked.',
    '',
    '## Feedback',
  ];
  queue.forEach((q, i) => {
    lines.push(
      `${i + 1}. **${q.name}** (${q.kind}) @ \`${q.path}\``,
      `   ${q.text}`,
    );
    if (q.rationale) lines.push(`   (original agent rationale: ${q.rationale})`);
  });
  return lines.join('\n');
}

export function sendFeedbackToAgent(queue: QueuedFeedback[], prTitle?: string) {
  const prompt = formatFeedbackPrompt(queue, prTitle);
  openAgentPanel();
  void agentSend(prompt);
}

/** Fetch host layer review policies (cached briefly per page). */
let _reviewPoliciesCache: { at: number; policies: Record<string, LayerReviewPolicy> } | null =
  null;

export async function fetchReviewPolicies(): Promise<Record<string, LayerReviewPolicy>> {
  if (_reviewPoliciesCache && Date.now() - _reviewPoliciesCache.at < 60_000) {
    return _reviewPoliciesCache.policies;
  }
  try {
    const r = await fetch(`${platformRoot()}/api/review_policies`, {
      headers: ideRequestHeaders(),
    });
    if (!r.ok) return _reviewPoliciesCache?.policies || {};
    const d = await r.json();
    const policies = (d.policies || d || {}) as Record<string, LayerReviewPolicy>;
    _reviewPoliciesCache = { at: Date.now(), policies };
    return policies;
  } catch {
    return _reviewPoliciesCache?.policies || {};
  }
}

export async function postJournalEntry(entry: {
  pr_id?: string | null;
  construct_path: string;
  construct_name: string;
  decision: string;
  rationale?: string | null;
  teaching_note?: string | null;
  risk?: string | null;
  package?: string | null;
}): Promise<void> {
  try {
    await fetch(`${platformRoot()}/api/journal`, {
      method: 'POST',
      headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
      body: JSON.stringify(entry),
    });
  } catch {
    /* journal is best-effort until host is fully wired */
  }
}

export async function fetchJournal(opts?: {
  construct?: string;
  pr_id?: string;
  q?: string;
  limit?: number;
}): Promise<unknown[]> {
  const u = new URL(`${platformRoot()}/api/journal`);
  if (opts?.construct) u.searchParams.set('construct', opts.construct);
  if (opts?.pr_id) u.searchParams.set('pr_id', opts.pr_id);
  if (opts?.q) u.searchParams.set('q', opts.q);
  if (opts?.limit) u.searchParams.set('limit', String(opts.limit));
  try {
    const r = await fetch(u.toString(), { headers: ideRequestHeaders() });
    if (!r.ok) return [];
    const d = await r.json();
    return Array.isArray(d) ? d : d.entries || d.items || [];
  } catch {
    return [];
  }
}

/** Learn-mode: load construct-scoped journal first, then PR, then global. */
export async function fetchLearnJournalWalk(opts: {
  construct?: string;
  pr_id?: string | null;
  limit?: number;
}): Promise<{
  construct: unknown[];
  pr: unknown[];
  global: unknown[];
  merged: unknown[];
}> {
  const limit = opts.limit ?? 40;
  const [construct, pr, global] = await Promise.all([
    opts.construct
      ? fetchJournal({ construct: opts.construct, limit })
      : Promise.resolve([]),
    opts.pr_id ? fetchJournal({ pr_id: opts.pr_id, limit }) : Promise.resolve([]),
    fetchJournal({ limit }),
  ]);
  const seen = new Set<string>();
  const merged: unknown[] = [];
  for (const list of [construct, pr, global]) {
    for (const e of list) {
      const id =
        (e as { id?: string }).id ||
        `${(e as { ts?: string }).ts}|${(e as { construct_name?: string }).construct_name}|${(e as { decision?: string }).decision}`;
      if (seen.has(id)) continue;
      seen.add(id);
      merged.push(e);
    }
  }
  return { construct, pr, global, merged };
}
