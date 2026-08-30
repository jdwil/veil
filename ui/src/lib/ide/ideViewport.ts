/**
 * IDE viewport — authoritative snapshot of every major pane for SessionFocus.
 *
 * UX is source of truth. Publishers (focusBridge, IdeApp, ReviewDock) patch
 * slices; flushIdeViewportToFocus() rebuilds SessionFocus.panes so the agent can
 * resolve "this", "the outline", "the review dock", etc.
 */
import { writable, get } from 'svelte/store';
import { patchFocus, type FocusPane } from '$lib/agent/focus';
import { agentPanelOpen, agentPanelMinimized } from '$lib/agent/runtimeAgentSession';

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
    };
    return next;
  });
  return next;
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

  // Deictic defaults from current construct selection.
  const construct = v.construct;
  const constructKind = v.constructKind;
  const changeId = null;
  const panel = v.primaryPane;

  const selection = v.construct
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
