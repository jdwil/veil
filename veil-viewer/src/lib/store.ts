import { writable } from 'svelte/store';
import { setPaletteStyles, type IrGraph, type IrNode, type PaletteEntry } from './types';

export const irGraph = writable<IrGraph | null>(null);
export const veilSource = writable<string>('');
export const currentParent = writable<number | null>(null);
export const breadcrumbs = writable<{ id: number | null; name: string }[]>([]);
export const loading = writable(true);
export const error = writable<string | null>(null);
export const selectedNodeId = writable<string | null>(null);
export const paletteConfig = writable<any[]>([]);
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
const CHECK_URL = `${API_BASE}/check`;
const EDIT_URL = `${API_BASE}/edit`;
const STUBS_URL = `${API_BASE}/stubs`;
const FILES_URL = `${API_BASE}/files`;
const SELECT_FILE_URL = `${API_BASE}/files/select`;

/** Loaded file metadata from the server. */
export interface VeilFileInfo {
  index: number;
  name: string;
  path: string;
  editable: boolean;
  active: boolean;
}

/** List of available files and the currently active one. */
export const availableFiles = writable<VeilFileInfo[]>([]);
export const activeFileName = writable<string>('');

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
    const res = await fetch(`${CHECK_URL}?target=${encodeURIComponent(t || 'rust')}`);
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

export async function fetchIr() {
  loading.set(true);
  error.set(null);
  try {
    const [irRes, srcRes, palRes, stubRes, filesRes] = await Promise.all([
      fetch(API_URL),
      fetch(SOURCE_URL),
      fetch(PALETTE_URL),
      fetch(STUBS_URL).catch(() => null),
      fetch(FILES_URL).catch(() => null),
    ]);
    if (!irRes.ok) throw new Error(`HTTP ${irRes.status}`);
    const data: IrGraph = await irRes.json();
    irGraph.set(data);

    if (stubRes && stubRes.ok) {
      stubs.set(await stubRes.json());
    }

    // Full dual-loop check (CHK-007)
    await fetchCheck();

    if (srcRes.ok) {
      veilSource.set(await srcRes.text());
    }

    if (palRes.ok) {
      const palette: PaletteEntry[] = await palRes.json();
      paletteConfig.set(palette);
      setPaletteStyles(palette);
    }

    // Load file list
    if (filesRes && filesRes.ok) {
      const files: VeilFileInfo[] = await filesRes.json();
      availableFiles.set(files);
      const active = files.find(f => f.active);
      if (active) activeFileName.set(active.name);
    }

    // Find root and determine entry point
    const root = data.nodes.find(n => n.kind === 'Solution');
    if (root) {
      const rootChildren = data.nodes.filter(n => n.metadata.parent === root.id);
      const flows = rootChildren.filter(n => n.kind === 'Flow');
      const nonFlows = rootChildren.filter(n => n.kind !== 'Flow');

      if (flows.length === 1 && nonFlows.every(n => n.metadata.annotations.includes('📦 package'))) {
        currentParent.set(flows[0].id);
        breadcrumbs.set([{ id: flows[0].id, name: flows[0].name }]);
      } else {
        currentParent.set(root.id);
        breadcrumbs.set([{ id: root.id, name: root.name }]);
      }
    }
  } catch (e) {
    error.set(e instanceof Error ? e.message : 'Failed to fetch IR');
  } finally {
    loading.set(false);
  }
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

/** Switch to a different loaded file by index. Re-fetches IR from the server. */
export async function selectFile(index: number) {
  loading.set(true);
  error.set(null);
  try {
    const res = await fetch(SELECT_FILE_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ index }),
    });
    if (!res.ok) throw new Error(`Failed to select file: HTTP ${res.status}`);
    const data: IrGraph = await res.json();
    irGraph.set(data);

    // Refresh source
    const srcRes = await fetch(SOURCE_URL);
    if (srcRes.ok) veilSource.set(await srcRes.text());

    // Update file list
    const filesRes = await fetch(FILES_URL);
    if (filesRes.ok) {
      const files: VeilFileInfo[] = await filesRes.json();
      availableFiles.set(files);
      const active = files.find(f => f.active);
      if (active) activeFileName.set(active.name);
    }

    // Re-run check for the newly active file
    await fetchCheck();

    // Reset navigation to root
    const root = data.nodes.find(n => n.kind === 'Solution');
    if (root) {
      currentParent.set(root.id);
      breadcrumbs.set([{ id: root.id, name: root.name }]);
    }
  } catch (e) {
    error.set(e instanceof Error ? e.message : 'Failed to switch file');
  } finally {
    loading.set(false);
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
