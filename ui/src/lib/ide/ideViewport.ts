/**
 * IDE viewport — authoritative snapshot of every major pane for SessionFocus.
 *
 * UX is source of truth. Publishers (focusBridge, IdeApp, PrWizard, ReviewDock)
 * patch slices; flushIdeViewportToFocus() rebuilds SessionFocus.panes so the
 * agent can resolve "this", "the wizard step", "the outline", etc.
 */
import { writable, get } from 'svelte/store';
import { patchFocus, type FocusPane } from '$lib/agent/focus';
import { agentPanelOpen, agentPanelMinimized } from '$lib/agent/runtimeAgentSession';

/** PR Wizard step the human is currently reading. */
export type PrWizardViewport = {
  open: boolean;
  phase?: string;
  changeId?: string | null;
  prTitle?: string | null;
  prStatus?: string | null;
  sourceBranch?: string | null;
  targetBranch?: string | null;
  /** Working-tree vs PR branch structural diff */
  diffSource?: string | null;
  step?: number;
  total?: number;
  itemName?: string | null;
  itemKind?: string | null;
  itemPath?: string | null;
  itemSubkind?: string | null;
  itemNodeKind?: string | null;
  container?: string | null;
  signature?: string | null;
  rationale?: string | null;
  decision?: string | null;
  /** Human-readable one-liner for focus */
  summary?: string | null;
};

export type ReviewDockViewport = {
  visible: boolean;
  tab?: string | null;
  expanded?: boolean;
};

export type IdeViewport = {
  project: string | null;
  file: string | null;
  fileKind?: string | null;
  /** Left rail: outline | changes */
  sidebarTab: string;
  construct: string | null;
  constructKind: string | null;
  constructSubkind?: string | null;
  selectionId?: string | null;
  diagnosticsCount: number;
  diagnosticsSample?: Array<{
    severity?: string;
    message?: string;
    node_name?: string | null;
  }>;
  reviewDock: ReviewDockViewport;
  prWizard: PrWizardViewport;
  /** Which pane the human last interacted with (deictic primary). */
  primaryPane: string | null;
  layout?: string | null;
};

const emptyViewport = (): IdeViewport => ({
  project: null,
  file: null,
  sidebarTab: 'outline',
  construct: null,
  constructKind: null,
  diagnosticsCount: 0,
  reviewDock: { visible: false },
  prWizard: { open: false },
  primaryPane: null,
});

export const ideViewport = writable<IdeViewport>(emptyViewport());

export function patchIdeViewport(partial: Partial<IdeViewport>): IdeViewport {
  let next = emptyViewport();
  ideViewport.update((prev) => {
    // Omit undefined keys so callers can patch slices without clearing primaryPane etc.
    const clean: Partial<IdeViewport> = {};
    for (const [k, v] of Object.entries(partial) as [keyof IdeViewport, IdeViewport[keyof IdeViewport]][]) {
      if (v !== undefined) (clean as Record<string, unknown>)[k as string] = v;
    }
    next = {
      ...prev,
      ...clean,
      reviewDock: clean.reviewDock
        ? { ...prev.reviewDock, ...clean.reviewDock }
        : prev.reviewDock,
      prWizard: clean.prWizard
        ? { ...prev.prWizard, ...clean.prWizard }
        : prev.prWizard,
    };
    return next;
  });
  return next;
}

function summarizeWizard(w: PrWizardViewport): string {
  if (!w.open) return '';
  if (w.phase === 'walk' && w.itemName) {
    const n = (w.step ?? 0) + 1;
    const t = w.total ?? 0;
    const kind = w.itemKind || 'change';
    return (
      `PR Wizard step ${n}/${t}: ${kind} \`${w.itemName}\`` +
      (w.signature ? ` — ${w.signature}` : '')
    );
  }
  return (
    `PR Wizard (${w.phase || 'open'})` + (w.prTitle ? `: ${w.prTitle}` : '')
  );
}

/**
 * Update PR Wizard slice and flush focus.
 * While open, wizard is primary for deictic references ("this change", "this method").
 */
export function publishPrWizardViewport(wiz: Partial<PrWizardViewport> & { open: boolean }) {
  const prev = get(ideViewport);
  const merged: PrWizardViewport = { ...prev.prWizard, ...wiz };
  merged.summary = summarizeWizard(merged) || null;

  let primaryPane = prev.primaryPane;
  if (merged.open) {
    primaryPane = 'pr-wizard';
  } else if (prev.primaryPane === 'pr-wizard') {
    primaryPane = 'canvas';
  }

  patchIdeViewport({
    prWizard: merged,
    primaryPane,
  });
  flushIdeViewportToFocus();
}

/** Mark which pane was last used (click/focus). */
export function setPrimaryIdePane(paneId: string) {
  patchIdeViewport({ primaryPane: paneId });
  flushIdeViewportToFocus();
}

function pane(
  id: string,
  label: string,
  summary: string,
  primary: boolean,
  details?: FocusPane['details']
): FocusPane {
  return { id, label, summary, primary, details };
}

/**
 * Rebuild SessionFocus.panes (+ core construct/file/change fields) from ideViewport.
 * Call before every agent turn and after any pane change.
 */
export function flushIdeViewportToFocus(): IdeViewport {
  const v = get(ideViewport);
  const primary = v.primaryPane;
  const agentOpen = get(agentPanelOpen);
  const agentMin = get(agentPanelMinimized);

  const panes: FocusPane[] = [];

  if (v.project) {
    panes.push(
      pane(
        'project',
        'Project',
        `IDE project \`${v.project}\`` + (v.file ? ` · file \`${v.file}\`` : ''),
        primary === 'project',
        {
          project: v.project,
          file: v.file,
          fileKind: v.fileKind ?? null,
          layout: v.layout ?? null,
        }
      )
    );
  }

  if (v.sidebarTab === 'changes') {
    panes.push(
      pane(
        'sidebar-changes',
        'Changes panel',
        'Left rail: Changes (session commits / CR list)',
        primary === 'sidebar-changes' || primary === 'changes',
        { tab: 'changes' }
      )
    );
  } else {
    const c = v.construct
      ? `selected \`${v.construct}\`` +
        (v.constructSubkind || v.constructKind
          ? ` (${v.constructSubkind || v.constructKind})`
          : '')
      : 'no selection';
    panes.push(
      pane(
        'outline',
        'Outline',
        `Left rail: Outline — ${c}`,
        primary === 'outline',
        {
          construct: v.construct,
          constructKind: v.constructKind,
          constructSubkind: v.constructSubkind ?? null,
        }
      )
    );
  }

  panes.push(
    pane(
      'canvas',
      'Canvas',
      v.construct
        ? `Graph/canvas focus: \`${v.construct}\`` +
            (v.constructKind ? ` (${v.constructKind})` : '')
        : 'Graph/canvas: nothing selected',
      primary === 'canvas' || primary === 'detail',
      {
        construct: v.construct,
        constructKind: v.constructKind,
        selectionId: v.selectionId ?? null,
      }
    )
  );

  if (v.diagnosticsCount > 0) {
    const sample = (v.diagnosticsSample ?? [])
      .slice(0, 3)
      .map(
        (d) =>
          `${d.severity ?? 'Issue'}${d.node_name ? ` @ ${d.node_name}` : ''}: ${d.message ?? ''}`
      )
      .join('; ');
    panes.push(
      pane(
        'diagnostics',
        'Diagnostics',
        `${v.diagnosticsCount} open diagnostic(s)` + (sample ? ` — ${sample}` : ''),
        primary === 'diagnostics',
        { count: v.diagnosticsCount }
      )
    );
  }

  if (v.reviewDock.visible) {
    const tab = v.reviewDock.tab || 'source';
    const exp = v.reviewDock.expanded !== false;
    panes.push(
      pane(
        'review-dock',
        'Review dock',
        exp
          ? `Bottom dock: ${tab}` + (v.file ? ` for \`${v.file}\`` : '')
          : 'Bottom dock: collapsed',
        primary === 'review-dock' || primary === 'source',
        {
          tab,
          expanded: exp,
          file: v.file,
        }
      )
    );
  }

  if (v.prWizard.open) {
    const w = v.prWizard;
    panes.push(
      pane(
        'pr-wizard',
        'PR Wizard',
        w.summary ||
          `PR Wizard open (${w.phase || '…'})` +
            (w.itemName ? ` · ${w.itemKind || 'item'} \`${w.itemName}\`` : ''),
        primary === 'pr-wizard' || !primary,
        {
          phase: w.phase ?? null,
          changeId: w.changeId ?? null,
          prTitle: w.prTitle ?? null,
          prStatus: w.prStatus ?? null,
          sourceBranch: w.sourceBranch ?? null,
          targetBranch: w.targetBranch ?? null,
          diffSource: w.diffSource ?? null,
          step: w.step != null ? w.step + 1 : null,
          total: w.total ?? null,
          itemName: w.itemName ?? null,
          itemKind: w.itemKind ?? null,
          itemPath: w.itemPath ?? null,
          itemSubkind: w.itemSubkind ?? null,
          signature: w.signature ?? null,
          rationale: w.rationale ?? null,
          decision: w.decision ?? null,
          container: w.container ?? null,
        }
      )
    );
  }

  if (agentOpen) {
    panes.push(
      pane(
        'agent',
        'Agent panel',
        agentMin ? 'Agent panel minimized (strip)' : 'Agent panel open (right dock)',
        primary === 'agent',
        { minimized: agentMin }
      )
    );
  }

  // Deictic defaults: wizard step construct wins while wizard is open
  const construct =
    v.prWizard.open && v.prWizard.itemName ? v.prWizard.itemName : v.construct;
  const constructKind =
    v.prWizard.open && (v.prWizard.itemNodeKind || v.prWizard.itemSubkind)
      ? v.prWizard.itemNodeKind || v.prWizard.itemSubkind
      : v.constructKind;
  const changeId = v.prWizard.open ? (v.prWizard.changeId ?? null) : null;
  const panel = v.prWizard.open ? 'pr-wizard' : v.primaryPane;

  const selection =
    v.prWizard.open && v.prWizard.itemName
      ? {
          kind: v.prWizard.itemNodeKind || v.prWizard.itemKind || 'diff-item',
          id: v.prWizard.itemPath || v.prWizard.itemName,
          label: v.prWizard.itemName,
        }
      : v.construct
        ? {
            kind: v.constructKind || 'construct',
            id: v.selectionId || v.construct,
            label: v.construct,
          }
        : null;

  patchFocus({
    project: v.project,
    route: v.project ? `/projects/${encodeURIComponent(v.project)}/ide` : undefined,
    file: v.file,
    construct: construct ?? null,
    constructKind: constructKind ?? null,
    selection,
    changeId,
    panel: panel ?? null,
    diagnostics:
      v.diagnosticsCount > 0
        ? {
            count: v.diagnosticsCount,
            sample: v.diagnosticsSample,
          }
        : { count: 0 },
    panes,
    primaryPane: panel ?? v.primaryPane,
  });

  return v;
}

/** Clear IDE viewport when leaving IDE route. */
export function clearIdeViewport() {
  ideViewport.set(emptyViewport());
  patchFocus({
    panes: [],
    primaryPane: null,
    panel: null,
    construct: null,
    constructKind: null,
    selection: null,
    file: null,
    changeId: null,
  });
}
