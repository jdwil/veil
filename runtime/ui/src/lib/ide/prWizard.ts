/**
 * PR Wizard — shared types + API for structural walk-through review.
 * Human-in-the-loop: approve each DiffItem or queue feedback for the agent.
 */
import { writable, get } from 'svelte/store';
import {
  apiHost,
  currentProjectParam,
  getCodingSessionId,
  ideApiBase,
  ideRequestHeaders,
  codingSessionMeta,
} from './store';
import { agentSend, openAgentPanel } from '$lib/agent/runtimeAgentSession';

export interface PathSegment {
  name: string;
  subkind?: string | null;
}

export interface DiffItem {
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
  container_path?: PathSegment[];
}

/** IR snapshot for review — fields, methods, body, intent */
export interface ConstructPeek {
  side: string;
  name: string;
  node_kind: string;
  subkind?: string | null;
  path?: string | null;
  signature?: string | null;
  fields?: string[];
  methods?: string[];
  body_preview?: string[];
  annotations?: string[];
  intent?: string | null;
}

export interface EditAnnotation {
  intent?: string | null;
  category?: string | null;
  criticality?: string | null;
}

export interface FileDiffHunk {
  header?: string;
  lines?: string[];
}

export interface FileDiff {
  path: string;
  status: string;
  hunks?: FileDiffHunk[];
  base_lines?: number;
  head_lines?: number;
}

export interface LayerReviewPolicy {
  strategy?: string;
  target?: string | null;
  fallback?: string | null;
  secondary?: string[];
  impact?: string[];
}

export interface StructDiff {
  base_label: string;
  head_label: string;
  items: DiffItem[];
  added: number;
  removed: number;
  changed: number;
  description?: string;
  changes?: unknown[];
  files_changed?: number;
  parse_notes?: string[];
  error?: string;
  item_annotations?: (EditAnnotation | null)[];
  item_peeks?: (ConstructPeek | null)[];
  item_peeks_base?: (ConstructPeek | null)[];
  /** Secondary git-style file diffs (not front-and-center). */
  file_diffs?: FileDiff[];
  /** Per-item IR graph blast radius (dependents / deps / container). */
  item_impact?: (string[] | null)[];
  /** Package `use` layers touched by this diff. */
  used_layers?: string[];
  /** Layer name → review presentation policy (from layer `review` blocks). */
  review_policies?: Record<string, LayerReviewPolicy>;
}

export interface PullRequest {
  id: string;
  title: string;
  description: string;
  jira_ticket?: string;
  source_branch: string;
  target_branch: string;
  author: string;
  status: string;
  created_at?: string;
  updated_at?: string;
  repo_id?: string;
}

export interface ReviewComment {
  id: string;
  pr_id?: string;
  author: string;
  construct_path?: string | null;
  body: string;
  created_at: string;
  resolved?: boolean;
}

export type ItemDecision = 'approve' | 'feedback' | 'skip' | null;

export type RiskLevel = 'critical' | 'high' | 'normal' | 'low';

export type PreviewDepth = 'peek' | 'il' | 'source';

export type WizardMode = 'review' | 'learn';

export interface WizardItemState {
  index: number;
  item: DiffItem;
  decision: ItemDecision;
  feedback: string;
  /** Optional teaching note attached on accept/reject (journal). */
  teachingNote: string;
  /** Matched agent rationale from PR description / commits / intent annotations */
  rationale: string | null;
  /** Head (or removed) construct snapshot for review */
  peek: ConstructPeek | null;
  /** Base snapshot for modified items */
  peekBase: ConstructPeek | null;
  sentToAgent: boolean;
  criticality: RiskLevel;
  annotation: EditAnnotation | null;
  /** Blast-radius labels (same container + related names). */
  impact: string[];
}

/** One walk step — groups field-level noise under a construct. */
export interface WizardGroup {
  key: string;
  name: string;
  path: string;
  risk: RiskLevel;
  children: WizardItemState[];
  expanded: boolean;
  /** Aggregate decision when all children agree; else null. */
  decision: ItemDecision;
  rationale: string | null;
  impact: string[];
}

export interface QueuedFeedback {
  index: number;
  path: string;
  name: string;
  kind: string;
  text: string;
  rationale?: string | null;
}

/** Whether the PR Wizard overlay is open. */
export const prWizardOpen = writable(false);
/** Optional pull-request id (null = session working-tree review). */
export const prWizardChangeId = writable<string | null>(null);

/** PR Wizard right-rail width (px) — drag-resizable like other IDE panes. */
const PR_WIZARD_WIDTH_KEY = 'veil.prWizard.width';
export const PR_WIZARD_MIN_WIDTH = 320;
export const PR_WIZARD_DEFAULT_WIDTH = 420;

export function prWizardMaxWidth(): number {
  if (typeof window === 'undefined') return 720;
  return Math.max(PR_WIZARD_MIN_WIDTH, Math.min(900, Math.floor(window.innerWidth * 0.65)));
}

export function clampPrWizardWidth(n: number): number {
  return Math.min(prWizardMaxWidth(), Math.max(PR_WIZARD_MIN_WIDTH, Math.round(n)));
}

function loadPrWizardWidth(): number {
  if (typeof localStorage === 'undefined') return PR_WIZARD_DEFAULT_WIDTH;
  const n = Number(localStorage.getItem(PR_WIZARD_WIDTH_KEY));
  if (!Number.isFinite(n) || n <= 0) return PR_WIZARD_DEFAULT_WIDTH;
  return clampPrWizardWidth(n);
}

export const prWizardWidth = writable(
  typeof window !== 'undefined' ? loadPrWizardWidth() : PR_WIZARD_DEFAULT_WIDTH
);

export function setPrWizardWidth(px: number) {
  const w = clampPrWizardWidth(px);
  prWizardWidth.set(w);
  try {
    localStorage.setItem(PR_WIZARD_WIDTH_KEY, String(w));
  } catch {
    /* ignore */
  }
}

/** Re-read persisted width (e.g. after mount / hydration). */
export function restorePrWizardWidth() {
  prWizardWidth.set(loadPrWizardWidth());
}

export function openPrWizard(prId?: string | null) {
  prWizardChangeId.set(prId ?? null);
  prWizardOpen.set(true);
}

export function closePrWizard() {
  prWizardOpen.set(false);
  // Lazy import to avoid circular deps at module init
  void import('./ideViewport').then(({ publishPrWizardViewport }) => {
    publishPrWizardViewport({ open: false });
  });
}

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

export function pathOf(item: DiffItem): string {
  if (item.path && item.name) {
    return item.path.includes(item.name) ? item.path : `${item.path}/${item.name}`;
  }
  return item.path || item.name || item.to_name || '(root)';
}

export function itemDisplayName(item: DiffItem): string {
  return item.name || item.to_name || item.from_name || pathOf(item);
}

export function itemKindLabel(kind: string): string {
  switch (kind) {
    case 'added':
      return 'Added';
    case 'removed':
      return 'Removed';
    case 'renamed':
      return 'Renamed';
    case 'signature_changed':
      return 'Signature';
    case 'body_changed':
      return 'Body';
    case 'annotations_changed':
      return 'Annotations';
    default:
      return kind.replaceAll('_', ' ');
  }
}

export function itemKindClass(kind: string): string {
  if (kind === 'added') return 'add';
  if (kind === 'removed') return 'rem';
  return 'chg';
}

export function containerLabel(item: DiffItem): string {
  const cp = item.container_path;
  if (cp && cp.length > 0) {
    return cp.map((s) => (s.subkind ? `${s.subkind} ${s.name}` : s.name)).join(' → ');
  }
  return item.path || '(root)';
}

/**
 * Extract per-construct rationales from agent PR description / commit messages.
 * Looks for markdown-ish sections: `### Name`, `- **Name**: why`, `path: why`.
 */
export function parseRationales(text: string): Map<string, string> {
  const map = new Map<string, string>();
  if (!text) return map;
  const lines = text.split('\n');
  let current: string | null = null;
  let buf: string[] = [];
  const flush = () => {
    if (current && buf.length) {
      map.set(current.toLowerCase(), buf.join('\n').trim());
    }
    current = null;
    buf = [];
  };
  for (const raw of lines) {
    const line = raw.trim();
    const h = line.match(/^#{1,4}\s+(.+)/);
    if (h) {
      flush();
      current = h[1].replace(/`/g, '').trim();
      continue;
    }
    const bullet = line.match(/^[-*]\s+\*?\*?([A-Za-z0-9_./:-]+)\*?\*?\s*[:—-]\s*(.+)$/);
    if (bullet) {
      flush();
      map.set(bullet[1].toLowerCase(), bullet[2].trim());
      continue;
    }
    if (current) buf.push(raw);
  }
  flush();
  // Whole description as package rationale
  if (map.size === 0 && text.trim().length > 20) {
    map.set('*', text.trim().slice(0, 2000));
  }
  return map;
}

export function matchRationale(item: DiffItem, rationales: Map<string, string>): string | null {
  const candidates = [
    item.name,
    item.to_name,
    item.from_name,
    pathOf(item),
    item.path,
  ].filter(Boolean) as string[];
  for (const c of candidates) {
    const hit = rationales.get(c.toLowerCase());
    if (hit) return hit;
    // partial: name contained in key or key in name
    for (const [k, v] of rationales) {
      if (k === '*') continue;
      if (c.toLowerCase().includes(k) || k.includes(c.toLowerCase())) return v;
    }
  }
  return rationales.get('*') ?? null;
}

/** Merge multiple rationale sources (later maps win on key conflict). */
export function mergeRationaleMaps(...maps: Map<string, string>[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of maps) {
    for (const [k, v] of m) {
      if (k && v) out.set(k, v);
    }
  }
  return out;
}

/**
 * Parse rationales from PR description + review comments (agent replies often
 * list `- **Construct**: why` after a rationale pass).
 */
export function rationalesFromPrTexts(
  description: string,
  comments?: ReviewComment[] | null
): Map<string, string> {
  const maps: Map<string, string>[] = [parseRationales(description || '')];
  if (comments?.length) {
    for (const c of comments) {
      const body = c.body || '';
      // Prefer agent replies and freeform bodies that look like rationale lists
      if (
        c.author === 'agent' ||
        body.includes('[pr-wizard:agent_reply]') ||
        body.includes('## Rationales') ||
        /[-*]\s+\*?\*?[A-Za-z]/.test(body)
      ) {
        maps.push(parseRationales(body));
      }
    }
  }
  return mergeRationaleMaps(...maps);
}

/**
 * Re-apply intents onto existing wizard steps **without** resetting decisions.
 * Prefer annotation/peek intents from a fresh diff; fall back to text maps.
 */
export function mergeRationalesIntoItems(
  items: WizardItemState[],
  rationales: Map<string, string>,
  nextDiff?: StructDiff | null
): WizardItemState[] {
  if (!items.length) return items;

  // Index next-diff intents by construct name for stable match even if order shifts
  const byName = new Map<string, { intent?: string | null; peek?: ConstructPeek | null }>();
  if (nextDiff?.items?.length) {
    for (let i = 0; i < nextDiff.items.length; i++) {
      const di = nextDiff.items[i];
      const name = (di.name || di.to_name || '').toLowerCase();
      if (!name) continue;
      const ann = nextDiff.item_annotations?.[i]?.intent ?? null;
      const peek = nextDiff.item_peeks?.[i] ?? null;
      const intent = ann || peek?.intent || null;
      if (intent || peek) {
        byName.set(name, { intent, peek });
      }
    }
  }

  let changed = false;
  const out = items.map((it, index) => {
    let rationale = it.rationale;
    let peek = it.peek;
    let peekBase = it.peekBase;

    // Same-index annotation (working-tree refresh)
    if (nextDiff) {
      const ann = nextDiff.item_annotations?.[index]?.intent ?? null;
      const p = nextDiff.item_peeks?.[index] ?? null;
      const pb = nextDiff.item_peeks_base?.[index] ?? null;
      const sameName =
        !nextDiff.items?.[index] ||
        (nextDiff.items[index].name || nextDiff.items[index].to_name) ===
          (it.item.name || it.item.to_name);
      if (sameName) {
        if (!rationale && ann) rationale = ann;
        if (!rationale && p?.intent) rationale = p.intent;
        if (p && (!peek || !peek.intent)) peek = { ...p, intent: p.intent || peek?.intent };
        if (pb) peekBase = pb;
      }
    }

    // Name-keyed match across the full next diff
    if (!rationale) {
      const name = (it.item.name || it.item.to_name || '').toLowerCase();
      const hit = name ? byName.get(name) : undefined;
      if (hit?.intent) rationale = hit.intent;
      if (hit?.peek && !peek?.intent) peek = hit.peek;
    }

    if (!rationale) {
      rationale = matchRationale(it.item, rationales);
    }

    if (rationale !== it.rationale || peek !== it.peek || peekBase !== it.peekBase) {
      changed = true;
      return { ...it, rationale, peek, peekBase };
    }
    return it;
  });
  return changed ? out : items;
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
  const r = await fetch(u.toString(), { headers: ideRequestHeaders() });
  if (!r.ok) throw new Error(`list changes HTTP ${r.status}`);
  const data = await r.json();
  return Array.isArray(data) ? data : data.pull_requests || data.items || [];
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
 * Load structural diff for the wizard.
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
        if (diff.items.length > 0) {
          return { diff, source: 'pr' };
        }
        return {
          diff,
          source: 'pr-empty',
          note:
            'This PR has no structural snapshot on its branch (often smoke/test PRs or never published). ' +
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
  const r = await fetch(`${ideApiBase()}/diff`, { headers: ideRequestHeaders() });
  if (!r.ok) throw new Error(`working-tree diff HTTP ${r.status}`);
  const diff = (await r.json()) as StructDiff;
  if (!Array.isArray(diff.items)) diff.items = [];
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

export async function mergeChangeApi(prId: string, slug?: string | null): Promise<void> {
  const r = await fetch(`${platformRoot()}/api/pull_requests/${prId}/merge`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({
      merger: 'operator',
      slug: slug || currentProjectParam() || '',
    }),
  });
  if (!r.ok) throw new Error(`merge HTTP ${r.status}: ${await r.text()}`);
}

export async function createAndSubmitPr(opts: {
  title: string;
  description: string;
  source_branch?: string;
  slug?: string;
}): Promise<PullRequest> {
  const slug = opts.slug || currentProjectParam() || '';
  const meta = get(codingSessionMeta) as Record<string, unknown> | null;
  const branch =
    opts.source_branch ||
    (meta?.branch_name as string) ||
    (meta?.draft_mode ? 'work' : undefined);

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

  const create = await fetch(`${platformRoot()}/api/pull_requests`, {
    method: 'POST',
    headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({
      title: opts.title,
      description: desc,
      slug,
      source_branch: branch,
      author: 'agent',
      jira_ticket: `VEIL-${Date.now().toString(36).toUpperCase()}`,
    }),
  });
  if (!create.ok) throw new Error(`create PR HTTP ${create.status}: ${await create.text()}`);
  const created = await create.json();
  const pr = (created.pull_request || created) as PullRequest;
  if (!pr.id) throw new Error('create PR returned no id');

  // Publish session worktree onto PR branch so structural diff is real.
  const pubBranch = branch || pr.source_branch || 'work';
  if (sid && pubBranch && pubBranch !== 'main') {
    try {
      await fetch(`${platformRoot()}/api/sessions/${sid}/publish-branch`, {
        method: 'POST',
        headers: { ...ideRequestHeaders(), 'Content-Type': 'application/json' },
        body: JSON.stringify({ branch_name: pubBranch, pr_id: pr.id }),
      });
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

  const sub = await fetch(`${platformRoot()}/api/pull_requests/${pr.id}/submit`, {
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
      ? `Address PR Wizard feedback on \`${prTitle}\` for project \`${project}\`.`
      : `Address PR Wizard review feedback for project \`${project}\`.`,
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

export function riskFromCriticality(c?: string | null): RiskLevel {
  const s = (c || '').toLowerCase();
  if (s === 'critical' || s === '3') return 'critical';
  if (s === 'high' || s === '2') return 'high';
  if (s === 'low' || s === '0') return 'low';
  return 'normal';
}

export function riskFromKind(kind: string): RiskLevel {
  const k = kind.toLowerCase();
  if (k === 'removed' || k.includes('delete')) return 'high';
  if (k === 'signature_changed' || k === 'renamed') return 'high';
  if (k === 'body_changed') return 'normal';
  if (k === 'annotations_changed') return 'low';
  if (k === 'added') return 'normal';
  return 'normal';
}

export function maxRisk(a: RiskLevel, b: RiskLevel): RiskLevel {
  const rank = { critical: 3, high: 2, normal: 1, low: 0 };
  return rank[a] >= rank[b] ? a : b;
}

export function riskLabel(r: RiskLevel): string {
  return r === 'critical' ? 'Critical' : r === 'high' ? 'High' : r === 'low' ? 'Low' : 'Normal';
}

export function buildWizardItems(
  items: DiffItem[],
  rationales: Map<string, string>,
  diff?: StructDiff | null
): WizardItemState[] {
  const allNames = items.map((it) => itemDisplayName(it)).filter(Boolean);
  return items.map((item, index) => {
    const peek = diff?.item_peeks?.[index] ?? null;
    const peekBase = diff?.item_peeks_base?.[index] ?? null;
    const ann = diff?.item_annotations?.[index] ?? null;
    const annIntent = ann?.intent ?? null;
    const fromPeek = peek?.intent ?? null;
    const matched = matchRationale(item, rationales);
    const rationale = annIntent || fromPeek || matched;
    const criticality = ann?.criticality
      ? riskFromCriticality(String(ann.criticality))
      : riskFromKind(item.kind);
    const impactFromApi = diff?.item_impact?.[index] ?? null;
    const impact =
      impactFromApi && impactFromApi.length
        ? impactFromApi
        : inferImpact(item, allNames, index);
    return {
      index,
      item,
      decision: null,
      feedback: '',
      teachingNote: '',
      rationale: rationale ?? null,
      peek,
      peekBase,
      sentToAgent: false,
      criticality,
      annotation: ann,
      impact,
    };
  });
}

/** Group DiffItems by construct identity (path + name); sort groups by max risk. */
export function buildWizardGroups(items: WizardItemState[]): WizardGroup[] {
  const map = new Map<string, WizardItemState[]>();
  for (const it of items) {
    const name = itemDisplayName(it.item) || `item-${it.index}`;
    const path = pathOf(it.item) || '';
    const key = `${path}::${name}`;
    const list = map.get(key) || [];
    list.push(it);
    map.set(key, list);
  }
  const groups: WizardGroup[] = [];
  for (const [key, children] of map) {
    let risk: RiskLevel = 'low';
    let rationale: string | null = null;
    const impact = new Set<string>();
    for (const c of children) {
      risk = maxRisk(risk, c.criticality);
      if (!rationale && c.rationale) rationale = c.rationale;
      for (const i of c.impact) impact.add(i);
    }
    const first = children[0];
    groups.push({
      key,
      name: itemDisplayName(first.item) || key,
      path: pathOf(first.item),
      risk,
      children,
      expanded: children.length > 1,
      decision: aggregateDecision(children),
      rationale,
      impact: [...impact],
    });
  }
  const rank = { critical: 0, high: 1, normal: 2, low: 3 };
  groups.sort((a, b) => rank[a.risk] - rank[b.risk] || a.name.localeCompare(b.name));
  return groups;
}

function aggregateDecision(children: WizardItemState[]): ItemDecision {
  if (!children.length) return null;
  if (children.every((c) => c.decision === 'approve')) return 'approve';
  if (children.some((c) => c.decision === 'feedback')) return 'feedback';
  if (children.every((c) => c.decision === 'skip')) return 'skip';
  return null;
}

export function syncGroupDecision(g: WizardGroup): WizardGroup {
  return { ...g, decision: aggregateDecision(g.children) };
}

function inferImpact(item: DiffItem, allNames: string[], selfIndex: number): string[] {
  // Client fallback only — prefer server item_impact (IR graph edges).
  const name = itemDisplayName(item);
  const container = (item.container_path || []).map((s) => s.name).join('/');
  const out: string[] = [];
  if (container) out.push(`in ${container}`);
  for (let i = 0; i < allNames.length; i++) {
    if (i === selfIndex) continue;
    const n = allNames[i];
    if (!n || n === name) continue;
    if (n.includes(name) || (name.length > 3 && name.includes(n))) out.push(n);
  }
  return out.slice(0, 8);
}

/** Layer-declared review presentation (from layer `review` blocks + heuristics). */
export type ReviewStrategy = 'structural' | 'component_sandbox' | 'file_diff';

export interface ReviewPresentation {
  strategy: ReviewStrategy;
  target?: string;
  fallback: ReviewStrategy;
  secondary: ReviewStrategy[];
  impact: string[];
  /** Which layer policy won (if any). */
  fromLayer?: string;
}

function asStrategy(s?: string | null): ReviewStrategy | null {
  const v = (s || '').toLowerCase().trim();
  if (v === 'structural' || v === 'component_sandbox' || v === 'file_diff') return v;
  return null;
}

/** Resolve review presentation from layer `review` policies + construct subkind. */
export function resolveReviewPresentation(opts: {
  layers?: string[];
  subkind?: string | null;
  nodeKind?: string | null;
  policies?: Record<string, LayerReviewPolicy> | null;
}): ReviewPresentation {
  const layers = (opts.layers || []).map((l) => l.toLowerCase());
  const policies = opts.policies || {};
  // Prefer the most specific layer policy (svelte* over base/ui).
  const order = [...layers].sort((a, b) => {
    const score = (n: string) =>
      n.includes('svelte') ? 3 : n === 'ui' || n.includes('react') ? 2 : n === 'base' ? 0 : 1;
    return score(b) - score(a);
  });
  for (const name of order) {
    const pol =
      policies[name] ||
      policies[Object.keys(policies).find((k) => k.toLowerCase() === name) || ''];
    if (!pol?.strategy) continue;
    const strategy = asStrategy(pol.strategy) || 'structural';
    const fallback = asStrategy(pol.fallback) || 'structural';
    const secondary = (pol.secondary || [])
      .map((s) => asStrategy(s))
      .filter((s): s is ReviewStrategy => !!s);
    return {
      strategy,
      target: pol.target || undefined,
      fallback,
      secondary: secondary.length ? secondary : ['file_diff'],
      impact: pol.impact?.length ? pol.impact : ['dependents'],
      fromLayer: name,
    };
  }
  // Heuristic when no policy map (working-tree without used_layers).
  const sk = (opts.subkind || '').toLowerCase();
  const nk = (opts.nodeKind || '').toLowerCase();
  const isUi =
    layers.some((l) => l.includes('svelte') || l.includes('react') || l === 'ui') ||
    sk.includes('page') ||
    sk.includes('component') ||
    nk.includes('view');
  if (isUi && (layers.some((l) => l.includes('svelte')) || sk.includes('component'))) {
    return {
      strategy: 'component_sandbox',
      target: 'svelte5',
      fallback: 'structural',
      secondary: ['file_diff'],
      impact: ['dependents'],
    };
  }
  return {
    strategy: 'structural',
    fallback: 'structural',
    secondary: ['file_diff'],
    impact: ['dependents'],
  };
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
