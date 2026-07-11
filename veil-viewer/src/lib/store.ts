import { writable } from 'svelte/store';
import { setPaletteStyles, type IrGraph, type IrNode, type PaletteEntry } from './types';
import type { PresentationModel } from './presentation';

export const irGraph = writable<IrGraph | null>(null);
export const veilSource = writable<string>('');
export const currentParent = writable<number | null>(null);
export const breadcrumbs = writable<{ id: number | null; name: string }[]>([]);
export const loading = writable(true);
export const error = writable<string | null>(null);
/** Bumped after IR load so the canvas always re-runs computeView (even if parent id is unchanged). */
export const viewRevision = writable(0);

/** Monotonic generation — cancels stale fetchIr/selectFile races. */
let loadGeneration = 0;

const FETCH_MS = 20_000;

async function fetchWithTimeout(
  input: RequestInfo | URL,
  init?: RequestInit,
  ms = FETCH_MS
): Promise<Response> {
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), ms);
  try {
    return await fetch(input, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(t);
  }
}
export const selectedNodeId = writable<string | null>(null);
export const paletteConfig = writable<any[]>([]);
/** Layer-driven views / nest rules from GET /api/presentation (LAY-002/003). */
export const presentationModel = writable<PresentationModel | null>(null);
/** A diagnostic from `/api/check` (mirrors veil_ir::Diagnostic). */
export interface Diagnostic {
  severity: 'Error' | 'Warning' | string;
  message: string;
  node_id?: number | null;
  node_name?: string | null;
  code?: string;
  constraint?: string;
  parent?: string | null;
  hint?: string | null;
  span_start?: number | null;
  span_end?: number | null;
}

export interface CheckResponse {
  diagnostics: Diagnostic[];
  error_count: number;
  warning_count: number;
  target: string;
  escape_hatch: {
    raw_surface: number;
    empty_adapter: number;
    external_call: number;
    json_boundary: number;
  };
  ok: boolean;
}

export const diagnostics = writable<Diagnostic[]>([]);
/** Last full check response metadata (counts, target, escape summary). */
export const checkMeta = writable<Omit<CheckResponse, 'diagnostics'> | null>(null);
/** Active codegen target for check (rust | typescript). */
export const checkTarget = writable<string>('rust');

const API_BASE = 'http://localhost:3001/api';
const API_URL = `${API_BASE}/ir`;
const SOURCE_URL = `${API_BASE}/source`;
const PALETTE_URL = `${API_BASE}/palette`;
const PRESENTATION_URL = `${API_BASE}/presentation`;
const CHECK_URL = `${API_BASE}/check`;
const EDIT_URL = `${API_BASE}/edit`;
const STUBS_URL = `${API_BASE}/stubs`;
const FILES_URL = `${API_BASE}/files`;
const SELECT_FILE_URL = `${API_BASE}/files/select`;
const PROJECT_URL = `${API_BASE}/project`;

/** Loaded file metadata from the server. */
export interface VeilFileInfo {
  index: number;
  name: string;
  path: string;
  editable: boolean;
  active: boolean;
  /** package | layer | stub (DSL-001) */
  kind?: 'package' | 'layer' | 'stub' | string;
}

/** Active IDE project (one root per serve session). */
export interface ActiveProject {
  name: string | null;
  path: string | null;
  projects_dir: string;
}

/** List of available files and the currently active one. */
export const availableFiles = writable<VeilFileInfo[]>([]);
export const activeFileName = writable<string>('');
/** Active file kind for chrome switching. */
export const activeFileKind = writable<string>('package');
/** Project root for this IDE session (runtime launches one serve per product). */
export const activeProject = writable<ActiveProject | null>(null);

/** External crate stubs (from .stub files), for the External palette section. */
export const stubs = writable<StubCrate[]>([]);

export interface StubMethod {
  name: string;
  params: [string, string][];
  return_type: string | null;
}
export interface StubStruct {
  name: string;
  methods: StubMethod[];
}
export interface StubImpl {
  target: string;
  methods: StubMethod[];
}
export interface StubCrate {
  name: string;
  version: string;
  structs: StubStruct[];
  impls: StubImpl[];
}

/** Whether the last edit is in flight (disables re-entrant saves). */
export const saving = writable(false);
/** Last edit error message, if any. */
export const saveError = writable<string | null>(null);

/** Fetch full check pipeline results into diagnostics store. */
export async function fetchCheck(target?: string): Promise<CheckResponse | null> {
  let t = target;
  if (!t) {
    // read current target without subscribing
    const unsub = checkTarget.subscribe((v) => {
      t = v;
    });
    unsub();
  }
  try {
    const res = await fetchWithTimeout(
      `${CHECK_URL}?target=${encodeURIComponent(t || 'rust')}`
    );
    if (!res.ok && res.status !== 422) return null;
    const data: CheckResponse = await res.json();
    diagnostics.set(data.diagnostics ?? []);
    const { diagnostics: _d, ...meta } = data;
    checkMeta.set(meta);
    return data;
  } catch {
    return null;
  }
}

function applyRootNavigation(data: IrGraph) {
  const root = data.nodes.find((n) => n.kind === 'Solution');
  if (!root) {
    currentParent.set(null);
    breadcrumbs.set([]);
    viewRevision.update((n) => n + 1);
    return;
  }
  const rootChildren = data.nodes.filter((n) => n.metadata.parent === root.id);
  const flows = rootChildren.filter((n) => n.kind === 'Flow');
  const nonFlows = rootChildren.filter((n) => n.kind !== 'Flow');

  let targetId = root.id;
  let crumb = { id: root.id, name: root.name };
  if (
    flows.length === 1 &&
    nonFlows.every((n) => n.metadata.annotations.includes('📦 package'))
  ) {
    targetId = flows[0].id;
    crumb = { id: flows[0].id, name: flows[0].name };
  }

  // Force subscriber fire even when parent id is unchanged across files
  // (both packages use node id 1 for Solution).
  currentParent.set(null);
  breadcrumbs.set([]);
  currentParent.set(targetId);
  breadcrumbs.set([crumb]);
  viewRevision.update((n) => n + 1);
}

export type LoadActiveOptions = {
  /** Keep breadcrumbs / drill-down / selection when possible (agent edits). */
  preserveNav?: boolean;
};

/** Core IR + panels load (no loading flag). Returns false if superseded. */
async function loadActiveFile(
  gen: number,
  opts: LoadActiveOptions = {}
): Promise<boolean> {
  const preserveNav = opts.preserveNav === true;
  let prevParent: number | null = null;
  let prevCrumbs: { id: number | null; name: string }[] = [];
  let prevSel: string | null = null;
  if (preserveNav) {
    const unsubP = currentParent.subscribe((v) => {
      prevParent = v;
    });
    unsubP();
    const unsubB = breadcrumbs.subscribe((v) => {
      prevCrumbs = v;
    });
    unsubB();
    const unsubS = selectedNodeId.subscribe((v) => {
      prevSel = v;
    });
    unsubS();
  }

  const [irRes, srcRes, palRes, presRes, stubRes, filesRes, projRes] = await Promise.all([
    fetchWithTimeout(API_URL),
    fetchWithTimeout(SOURCE_URL),
    fetchWithTimeout(PALETTE_URL),
    fetchWithTimeout(PRESENTATION_URL).catch(() => null),
    fetchWithTimeout(STUBS_URL).catch(() => null),
    fetchWithTimeout(FILES_URL).catch(() => null),
    fetchWithTimeout(PROJECT_URL).catch(() => null),
  ]);
  if (gen !== loadGeneration) return false;

  if (!irRes.ok) {
    const body = await irRes.text().catch(() => '');
    const detail = body.trim().slice(0, 400);
    throw new Error(
      detail ? `HTTP ${irRes.status}: ${detail}` : `HTTP ${irRes.status}`
    );
  }
  const data: IrGraph = await irRes.json();
  if (gen !== loadGeneration) return false;

  irGraph.set(data);
  if (!preserveNav) {
    selectedNodeId.set(null);
  }

  if (stubRes && stubRes.ok) {
    stubs.set(await stubRes.json());
  }

  // Check: await when preserving nav (agent edit — need live error badge);
  // otherwise fire-and-forget so first paint isn't blocked on large packages.
  const checkPromise = fetchCheck();
  if (!preserveNav) {
    void checkPromise;
  }

  if (srcRes.ok) {
    veilSource.set(await srcRes.text());
  }

  if (palRes.ok) {
    const palette: PaletteEntry[] = await palRes.json();
    paletteConfig.set(palette);
    setPaletteStyles(palette);
  }

  if (presRes && presRes.ok) {
    presentationModel.set(await presRes.json());
  } else {
    presentationModel.set(null);
  }

  if (filesRes && filesRes.ok) {
    const files: VeilFileInfo[] = await filesRes.json();
    availableFiles.set(files);
    const active = files.find((f) => f.active);
    if (active) {
      activeFileName.set(active.name);
      activeFileKind.set(active.kind || 'package');
    }
  }

  if (projRes && projRes.ok) {
    activeProject.set(await projRes.json());
  }

  // Generated code is optional (can be slow); don't block UI
  void fetchWithTimeout(`${API_BASE}/generated`)
    .then(async (r) => {
      if (gen !== loadGeneration || !r.ok) return;
      generatedCode.set(await r.json());
    })
    .catch(() => {});

  if (preserveNav) {
    const parentStill =
      prevParent == null || data.nodes.some((n) => n.id === prevParent);
    if (parentStill && prevParent != null) {
      currentParent.set(prevParent);
      breadcrumbs.set(
        prevCrumbs.filter(
          (c) => c.id == null || data.nodes.some((n) => n.id === c.id)
        )
      );
    } else {
      applyRootNavigation(data);
    }
    if (prevSel && data.nodes.some((n) => String(n.id) === prevSel)) {
      selectedNodeId.set(prevSel);
    } else {
      selectedNodeId.set(null);
    }
    viewRevision.update((n) => n + 1);
    await checkPromise;
  } else {
    applyRootNavigation(data);
  }
  return true;
}

export async function fetchIr() {
  const gen = ++loadGeneration;
  loading.set(true);
  error.set(null);
  try {
    await loadActiveFile(gen);
  } catch (e) {
    if (gen === loadGeneration) {
      const msg =
        e instanceof Error
          ? e.name === 'AbortError'
            ? `Timed out talking to API at ${API_BASE} (is veil serve running?)`
            : e.message
          : 'Failed to fetch IR';
      error.set(msg);
    }
  } finally {
    if (gen === loadGeneration) loading.set(false);
  }
}

/**
 * Soft reload after agent / edit tools — no full-page loading flash, keep nav.
 * Prefer this when the server already applied source changes in-process.
 */
export async function refreshAfterEdit(): Promise<void> {
  const gen = ++loadGeneration;
  error.set(null);
  try {
    await loadActiveFile(gen, { preserveNav: true });
  } catch (e) {
    if (gen === loadGeneration) {
      const msg =
        e instanceof Error
          ? e.name === 'AbortError'
            ? `Timed out talking to API at ${API_BASE}`
            : e.message
          : 'Failed to refresh after edit';
      error.set(msg);
    }
  }
}

/** Last SSE revision we applied — skip the subscribe snapshot once. */
let lastSseRevision: number | null = null;
let sse: EventSource | null = null;
let sseRefreshTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Subscribe to `GET /api/events` so agent mid-turn writes update the badge
 * without waiting for the HTTP turn response.
 */
export function startRevisionWatch(): () => void {
  stopRevisionWatch();
  try {
    sse = new EventSource(`${API_BASE}/events`);
  } catch {
    return () => {};
  }
  const onRevision = (ev: MessageEvent) => {
    try {
      const data = JSON.parse(String(ev.data || '{}')) as {
        revision?: number;
        reason?: string;
      };
      const rev = data.revision;
      if (typeof rev !== 'number') return;
      if (lastSseRevision === null) {
        // First event is the subscribe snapshot — don't force a reload.
        lastSseRevision = rev;
        return;
      }
      if (rev === lastSseRevision) return;
      lastSseRevision = rev;
      // Debounce bursty multi-tool writes
      if (sseRefreshTimer) clearTimeout(sseRefreshTimer);
      sseRefreshTimer = setTimeout(() => {
        void refreshAfterEdit();
      }, 120);
    } catch {
      /* ignore malformed */
    }
  };
  sse.addEventListener('revision', onRevision as EventListener);
  sse.onmessage = onRevision; // fallback if event name stripped
  return stopRevisionWatch;
}

export function stopRevisionWatch(): void {
  if (sseRefreshTimer) {
    clearTimeout(sseRefreshTimer);
    sseRefreshTimer = null;
  }
  if (sse) {
    sse.close();
    sse = null;
  }
  lastSseRevision = null;
}

export function drillDown(node: IrNode) {
  currentParent.set(node.id);
  breadcrumbs.update(bc => [...bc, { id: node.id, name: node.name }]);
}

export function navigateTo(id: number | null) {
  currentParent.set(id);
  breadcrumbs.update(bc => {
    const idx = bc.findIndex(b => b.id === id);
    return idx >= 0 ? bc.slice(0, idx + 1) : bc;
  });
}

/** Switch to a different loaded file by index. Re-fetches IR + all panels (UX-011). */
export async function selectFile(index: number) {
  const gen = ++loadGeneration;
  loading.set(true);
  error.set(null);
  try {
    const res = await fetchWithTimeout(SELECT_FILE_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ index }),
    });
    if (!res.ok) {
      const body = await res.text().catch(() => '');
      const detail = body.trim().slice(0, 400);
      throw new Error(
        detail
          ? `Failed to select file: HTTP ${res.status}: ${detail}`
          : `Failed to select file: HTTP ${res.status}`
      );
    }
    // Body is IR for the new active file — discard; loadActiveFile re-fetches consistently.
    await res.text().catch(() => '');
    if (gen !== loadGeneration) return;
    await loadActiveFile(gen);
  } catch (e) {
    if (gen === loadGeneration) {
      const msg =
        e instanceof Error
          ? e.name === 'AbortError'
            ? `Timed out selecting file (API ${API_BASE})`
            : e.message
          : 'Failed to switch file';
      error.set(msg);
    }
  } finally {
    if (gen === loadGeneration) loading.set(false);
  }
}

/**
 * Select a graph node (and drill its parent chain) from a diagnostic.
 * Prefers `node_id`; falls back to matching `node_name`.
 */
export function focusDiagnostic(diag: Diagnostic) {
  let graph: IrGraph | null = null;
  const unsub = irGraph.subscribe((g) => {
    graph = g;
  });
  unsub();
  if (!graph) return;

  let node: IrNode | undefined;
  if (diag.node_id != null) {
    node = graph.nodes.find((n) => n.id === diag.node_id);
  }
  if (!node && diag.node_name) {
    node = graph.nodes.find((n) => n.name === diag.node_name);
  }
  if (!node) return;

  // Build breadcrumb path from root → parent of node
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const chain: { id: number | null; name: string }[] = [];
  let walk: IrNode | undefined = node;
  const seen = new Set<number>();
  while (walk && !seen.has(walk.id)) {
    seen.add(walk.id);
    chain.push({ id: walk.id, name: walk.name });
    const parentId = walk.metadata.parent;
    walk = parentId != null ? byId.get(parentId) : undefined;
  }
  chain.reverse();

  // Navigate to the node's parent scope so the node is visible as a child
  const parentId = node.metadata.parent ?? null;
  if (parentId != null) {
    const parentChain = chain.filter((c) => c.id !== node!.id);
    breadcrumbs.set(
      parentChain.length > 0
        ? parentChain
        : [{ id: parentId, name: byId.get(parentId)?.name ?? '…' }]
    );
    currentParent.set(parentId);
  } else {
    breadcrumbs.set(chain.length ? [chain[0]] : []);
    currentParent.set(node.id);
  }
  selectedNodeId.set(String(node.id));
}

/** Get children of a given parent node */
export function getChildren(graph: IrGraph, parentId: number | null): IrNode[] {
  if (parentId === null) {
    return graph.nodes.filter(n => n.metadata.parent === null);
  }
  return graph.nodes.filter(n => n.metadata.parent === parentId);
}

/** Generated Rust files (path → content), refreshed after each successful edit. */
export const generatedCode = writable<Record<string, string> | null>(null);

/**
 * A structured edit operation, keyed by the target node's **AST span start**
 * (`node.span.start` / `node.data.spanStart`). Mirrors veil-ir `EditOp`
 * (serde tag = `"op"`, snake_case).
 *
 * Edits are **not** keyed by ephemeral IR node ids. After a successful save the
 * server returns a fresh IR; use the new spans for subsequent edits.
 *
 * `set_body` lines are VEIL expression source; the server parses them into real
 * `Expr` AST (invalid text fails the request and does not write the file).
 */
export type EditOp =
  | { op: 'rename'; span_start: number; name: string }
  | { op: 'set_annotations'; span_start: number; annotations: string[] }
  | { op: 'set_fields'; span_start: number; fields: { name: string; type: string }[] }
  | {
      op: 'set_methods';
      span_start: number;
      methods: {
        name: string;
        params: { name: string; type: string }[];
        return_type: string;
      }[];
    }
  | {
      op: 'create_construct';
      parent_span: number;
      keyword: string;
      name: string;
      target?: string;
    }
  | { op: 'set_body'; span_start: number; body: string[] }
  /** Remove construct / step / free-fn by AST span start (SER-006). */
  | { op: 'delete_construct'; span_start: number };

/**
 * Persist a batch of structured edits to the server. The server applies them
 * to the AST, re-serializes + validates, writes the .veil file, and returns
 * fresh source / IR / generated code, which we push into the stores so every
 * panel (graph, source, code preview) updates live.
 *
 * Returns true on success; on failure sets `saveError` and leaves state intact.
 */
export async function saveEdits(edits: EditOp[]): Promise<boolean> {
  if (edits.length === 0) return true;
  saving.set(true);
  saveError.set(null);
  try {
    const res = await fetch(EDIT_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ edits }),
    });
    if (!res.ok) {
      const msg = await res.text();
      saveError.set(msg || `HTTP ${res.status}`);
      return false;
    }
    const data: {
      source: string;
      ir: IrGraph;
      generated: Record<string, string>;
      diagnostics?: Diagnostic[];
    } = await res.json();
    irGraph.set(data.ir);
    veilSource.set(data.source);
    generatedCode.set(data.generated);
    if (data.diagnostics) {
      diagnostics.set(data.diagnostics);
    } else {
      await fetchCheck();
    }
    return true;
  } catch (e) {
    saveError.set(e instanceof Error ? e.message : 'Save failed');
    return false;
  } finally {
    saving.set(false);
  }
}
