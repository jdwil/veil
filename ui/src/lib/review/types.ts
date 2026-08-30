/**
 * Shared review types — structural diff, PR, and review-step models.
 *
 * These are the live types the /review ceremony depends on. They were formerly
 * housed in the misnamed `$lib/ide/prWizard.ts`; the PR-Wizard UX was removed
 * (2026-08-17) but these shared contracts remain in use.
 */

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
  /** Session dirty flag (from /diff) — false when agent reports clean. */
  uncommitted?: boolean;
  /** True when all-adds walk with no baseline would invent phantom review steps. */
  phantom_full_add?: boolean;
  session_id?: string;
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

/** One review step — groups field-level noise under a construct. */
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
