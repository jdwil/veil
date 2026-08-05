import { get, writable } from 'svelte/store';
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
/** Open the floating diagnostics list (tree badge / focusDiagnostic). */
export const diagnosticsPanelOpen = writable(false);
/** Last full check response metadata (counts, target, escape summary). */
export const checkMeta = writable<Omit<CheckResponse, 'diagnostics'> | null>(null);
/** Active codegen target for check (rust | typescript). */
export const checkTarget = writable<string>('rust');

/**
 * Host origin for the API.
 * Product default: same-origin ProductHost (runtime shell or /viewer).
 * Override: `?api=` or localStorage `veil-api-host`.
 */
export function apiHost(): string {
  if (typeof window === 'undefined') return '';
  const q = new URLSearchParams(window.location.search).get('api');
  if (q) return q.replace(/\/$/, '');
  try {
    const saved = localStorage.getItem('veil-api-host');
    if (saved) return saved.replace(/\/$/, '');
  } catch {
    /* ignore */
  }
  // Native shell IDE + same-origin viewer always hit this host's /api.
  return window.location.origin;
}

export function setApiHost(host: string) {
  try {
    localStorage.setItem('veil-api-host', host.replace(/\/$/, ''));
  } catch {
    /* ignore */
  }
}

/**
 * Active product project for the IDE.
 * - Native shell: `/projects/{slug}/ide`
 * - Legacy viewer: `?project=`
 * - Module override via `setNativeProjectSlug` (preferred when mounted as component)
 */
let nativeProjectSlug: string | null = null;

export function setNativeProjectSlug(slug: string | null) {
  nativeProjectSlug =
    slug && /^[a-zA-Z0-9_-]+$/.test(slug) ? slug : null;
}

export function currentProjectParam(): string | null {
  if (nativeProjectSlug) return nativeProjectSlug;
  if (typeof window === 'undefined') return null;
  // Native runtime shell route
  const path = window.location.pathname;
  const m = path.match(/\/projects\/([a-zA-Z0-9_-]+)\/ide\/?$/);
  if (m) return m[1];
  const p = new URLSearchParams(window.location.search).get('project');
  if (p && /^[a-zA-Z0-9_-]+$/.test(p)) return p;
  return null;
}

/** True when IDE is mounted inside runtime shell (not standalone agent chrome). */
export function isNativeShellIde(): boolean {
  if (nativeProjectSlug) return true;
  if (typeof window === 'undefined') return false;
  return /\/projects\/[a-zA-Z0-9_-]+\/ide\/?$/.test(window.location.pathname);
}

/**
 * IDE mode from `?mode=`.
 * - omit / full → dual-loop engineer IDE
 * - `flow` | `reaction` → layer flow composer (minimal shell)
 *
 * Optional overrides (any boolean shell flag):
 *   `?showAgentRail=0` `?showTopBar=1` etc. (0/false/off → false; 1/true/on → true)
 */
export function currentIdeMode(): string | null {
  if (typeof window === 'undefined') return null;
  const m = new URLSearchParams(window.location.search).get('mode');
  if (m && /^[a-zA-Z0-9_-]+$/.test(m)) return m.toLowerCase();
  return null;
}

/**
 * Context API host for meta-type field lookups (callables, types, etc.).
 * `?context_api=http://host:port` points at the host project that provides
 * available repos/ports/operations. Falls back to main apiHost().
 */
export function contextApiHost(): string {
  if (typeof window === 'undefined') return apiHost();
  const ctx = new URLSearchParams(window.location.search).get('context_api');
  if (ctx) return ctx.replace(/\/$/, '');
  // In flow mode, default to port 3001 (host project) if no explicit context_api
  if (isFlowComposerMode()) {
    const origin = window.location.origin;
    // If we're on the viewer dev server, the host project is likely on 3001
    return 'http://127.0.0.1:3001';
  }
  return apiHost();
}

/** Layer for flow composer: `?layer=reaction` (default `reaction` when mode=flow|reaction). */
export function flowLayerParam(): string | null {
  if (typeof window === 'undefined') return null;
  const l = new URLSearchParams(window.location.search).get('layer');
  if (l && /^[a-zA-Z0-9_-]+$/.test(l)) return l.toLowerCase();
  const m = currentIdeMode();
  if (m === 'flow' || m === 'reaction') return 'reaction';
  return null;
}

/** Reaction palette lock / use-reaction enforcement (layer is reaction). */
export function isReactionIdeMode(): boolean {
  const m = currentIdeMode();
  if (m === 'reaction') return true;
  if (m === 'flow' && (flowLayerParam() ?? 'reaction') === 'reaction') return true;
  // Infer when hub project is reaction and mode omitted (embed default)
  if (!m && currentProjectParam() === 'reaction') return true;
  return false;
}

export function isFlowComposerMode(): boolean {
  const m = currentIdeMode();
  if (m === 'flow' || m === 'reaction') return true;
  if (!m && currentProjectParam() === 'reaction') return true;
  return false;
}

/**
 * Shell chrome — every dual-loop feature is a separate flag so embeds can
 * toggle pieces later without inventing new modes.
 *
 * Preset `mode=flow|reaction`: palette (one layer) + canvas + props + agent.
 * No top bar, layers UI, source/review/dev chrome, or drill-down.
 */
export interface EmbedShellConfig {
  mode: 'full' | 'flow';
  /** Only constructs from these layer names (empty = all loaded layers). */
  paletteLayers: string[];
  showTopBar: boolean;
  showOutline: boolean;
  showDiff: boolean;
  showInfraToggle: boolean;
  showCriticalToggle: boolean;
  showDevToolbar: boolean;
  showReviewDock: boolean;
  showCodePreview: boolean;
  showAgentRail: boolean;
  showViewBar: boolean;
  showGroupTabs: boolean;
  showScopeBar: boolean;
  showDiagnostics: boolean;
  showMiniMap: boolean;
  showThemeToggle: boolean;
  showFlowControls: boolean;
  /** Double-click drill into nested IR */
  allowDrillDown: boolean;
  /** Drag from node handle → pick palette construct to attach */
  attachPickerOnConnect: boolean;
}

function queryBool(key: string): boolean | null {
  if (typeof window === 'undefined') return null;
  const v = new URLSearchParams(window.location.search).get(key);
  if (v == null) return null;
  if (v === '0' || v === 'false' || v === 'off' || v === 'no') return false;
  if (v === '1' || v === 'true' || v === 'on' || v === 'yes') return true;
  return null;
}

function applyQueryOverrides(cfg: EmbedShellConfig): EmbedShellConfig {
  const keys: (keyof EmbedShellConfig)[] = [
    'showTopBar',
    'showOutline',
    'showDiff',
    'showInfraToggle',
    'showCriticalToggle',
    'showDevToolbar',
    'showReviewDock',
    'showCodePreview',
    'showAgentRail',
    'showViewBar',
    'showGroupTabs',
    'showScopeBar',
    'showDiagnostics',
    'showMiniMap',
    'showThemeToggle',
    'showFlowControls',
    'allowDrillDown',
    'attachPickerOnConnect',
  ];
  const out = { ...cfg };
  for (const k of keys) {
    const b = queryBool(k);
    if (b !== null) (out as Record<string, unknown>)[k] = b;
  }
  return out;
}

export function embedShellConfig(): EmbedShellConfig {
  let cfg: EmbedShellConfig;
  if (isFlowComposerMode()) {
    const layer = flowLayerParam() || 'reaction';
    cfg = {
      mode: 'flow',
      paletteLayers: [layer],
      // REALLY basic: palette | graph | props-on-select | agent
      showTopBar: false,
      showOutline: false,
      showDiff: false,
      showInfraToggle: false,
      showCriticalToggle: false,
      showDevToolbar: false,
      showReviewDock: false,
      showCodePreview: false,
      showAgentRail: true,
      showViewBar: false,
      showGroupTabs: false,
      showScopeBar: false,
      showDiagnostics: false,
      showMiniMap: false,
      showThemeToggle: false,
      showFlowControls: false,
      allowDrillDown: false,
      attachPickerOnConnect: true,
    };
  } else {
    cfg = {
      mode: 'full',
      paletteLayers: [],
      showTopBar: true,
      showOutline: true,
      showDiff: true,
      showInfraToggle: true,
      showCriticalToggle: true,
      showDevToolbar: true,
      showReviewDock: true,
      showCodePreview: true,
      showAgentRail: true,
      showViewBar: true,
      showGroupTabs: true,
      showScopeBar: true,
      showDiagnostics: true,
      showMiniMap: true,
      showThemeToggle: true,
      showFlowControls: true,
      allowDrillDown: true,
      attachPickerOnConnect: false,
    };
  }
  // Runtime shell owns AgentDock + global theme — hide IDE-local chrome.
  if (isNativeShellIde()) {
    cfg.showAgentRail = false;
    cfg.showThemeToggle = false;
  }
  return applyQueryOverrides(cfg);
}

/**
 * Apply layer-declared IDE constraints to an EmbedShellConfig.
 * Called after the presentation model is loaded — merges `ide` constraints
 * from `GET /api/presentation` on top of the base config.
 *
 * Query-string overrides (`?showX=`) still take precedence (applied after this).
 */
export function applyIdeConstraints(
  cfg: EmbedShellConfig,
  ide: import('./presentation').IdeConstraints,
): EmbedShellConfig {
  const out = { ...cfg };

  // Map hide/show feature names to config keys
  const featureMap: Record<string, keyof EmbedShellConfig> = {
    palette: 'showOutline', // palette doesn't have a direct flag; map to outline as proxy
    outline: 'showOutline',
    diff: 'showDiff',
    infraToggle: 'showInfraToggle',
    criticalToggle: 'showCriticalToggle',
    devToolbar: 'showDevToolbar',
    reviewDock: 'showReviewDock',
    codePreview: 'showCodePreview',
    agentRail: 'showAgentRail',
    viewBar: 'showViewBar',
    groupTabs: 'showGroupTabs',
    scopeBar: 'showScopeBar',
    diagnostics: 'showDiagnostics',
    miniMap: 'showMiniMap',
    themeToggle: 'showThemeToggle',
    flowControls: 'showFlowControls',
  };

  if (ide.hide) {
    for (const feat of ide.hide) {
      const key = featureMap[feat];
      if (key) (out as Record<string, unknown>)[key] = false;
    }
  }
  if (ide.show) {
    for (const feat of ide.show) {
      const key = featureMap[feat];
      if (key) (out as Record<string, unknown>)[key] = true;
    }
  }
  if (ide.drill_depth === 0) {
    out.allowDrillDown = false;
  }

  return applyQueryOverrides(out);
}

/** Durable coding session id (X-Veil-Session-Id) — shared with runtime agent. */
const CODING_SESSION_KEY = 'veil.coding.sessionId';

export function getCodingSessionId(): string | null {
  if (typeof localStorage === 'undefined') return null;
  try {
    return localStorage.getItem(CODING_SESSION_KEY);
  } catch {
    return null;
  }
}

export function setCodingSessionId(id: string | null) {
  if (typeof localStorage === 'undefined') return;
  try {
    if (id) localStorage.setItem(CODING_SESSION_KEY, id);
    else localStorage.removeItem(CODING_SESSION_KEY);
  } catch {
    /* ignore */
  }
}

function applySessionPayload(data: {
  session?: {
    session_id?: string;
    slug?: string;
    revision?: number;
    draft_mode?: boolean;
  };
  work_dir?: string;
}) {
  const s = data?.session;
  if (!s?.session_id) return;
  setCodingSessionId(s.session_id);
  codingSessionRevision.set(typeof s.revision === 'number' ? s.revision : null);
  codingSessionMeta.set({
    session_id: s.session_id,
    slug: s.slug || '',
    revision: s.revision ?? 0,
    draft_mode: s.draft_mode,
    work_dir: data.work_dir,
  });
  sessionSaveState.set('ready');
  sessionSaveDetail.set(null);
}

/** Ensure a durable session for the current project (POST /api/sessions). */
/** In-flight ensure — coalesce concurrent IDE + agent opens. */
let ensureInflight: Promise<string | null> | null = null;

export async function ensureCodingSession(slug?: string | null): Promise<string | null> {
  const project = slug || currentProjectParam();
  if (!project) return getCodingSessionId();

  // Fast path: already bound for this slug
  const existing = getCodingSessionId();
  const meta = get(codingSessionMeta);
  if (existing && meta?.slug === project && meta.session_id === existing) {
    sessionSaveState.set('ready');
    return existing;
  }

  if (ensureInflight) return ensureInflight;

  ensureInflight = (async () => {
    sessionSaveState.set('ensuring');
    try {
      if (existing) {
        try {
          const res = await fetch(
            `${hubApiBase()}/sessions/${encodeURIComponent(existing)}/attach`,
            { method: 'POST' },
          );
          if (res.ok) {
            const data = await res.json();
            if (data?.session?.slug === project) {
              applySessionPayload(data);
              return existing;
            }
          }
        } catch {
          /* create below */
        }
      }
      // Create (server sticky may still re-use) — avoid listing full DDB when possible
      const res = await fetch(`${hubApiBase()}/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ slug: project }),
      });
      if (!res.ok) {
        sessionSaveState.set('error');
        sessionSaveDetail.set(`session create failed (${res.status})`);
        return getCodingSessionId();
      }
      const data = await res.json();
      applySessionPayload(data);
      return (data?.session?.session_id as string) || null;
    } catch (e) {
      sessionSaveState.set('error');
      sessionSaveDetail.set(e instanceof Error ? e.message : 'session error');
      return getCodingSessionId();
    } finally {
      ensureInflight = null;
    }
  })();

  return ensureInflight;
}

/** Headers for IDE API calls (mode + layer scope for palette / write locks). */
export function ideRequestHeaders(extra?: Record<string, string>): Record<string, string> {
  const h: Record<string, string> = { ...(extra || {}) };
  const mode = currentIdeMode();
  if (mode) h['X-Veil-Mode'] = mode;
  else if (isFlowComposerMode()) h['X-Veil-Mode'] = 'flow';
  const layer = flowLayerParam();
  if (layer) h['X-Veil-Layer'] = layer;
  const sid = getCodingSessionId();
  if (sid) h['X-Veil-Session-Id'] = sid;
  return h;
}

/** Debounced durable autosave for free-text editors (session workspace). */
let autosaveTimer: ReturnType<typeof setTimeout> | null = null;
let savedClearTimer: ReturnType<typeof setTimeout> | null = null;

export function scheduleAutosave(file: string, content: string, delayMs = 1500) {
  if (autosaveTimer) clearTimeout(autosaveTimer);
  sessionSaveState.set('saving');
  sessionSaveDetail.set(file);
  autosaveTimer = setTimeout(() => {
    void postAutosave(file, content);
  }, delayMs);
}

export async function postAutosave(file: string, content: string): Promise<boolean> {
  let sid = getCodingSessionId();
  if (!sid) {
    sid = await ensureCodingSession();
  }
  if (!sid) {
    sessionSaveState.set('error');
    sessionSaveDetail.set('no coding session');
    return false;
  }
  sessionSaveState.set('saving');
  try {
    const res = await fetch(`${ideApiBase()}/autosave`, {
      method: 'POST',
      headers: ideRequestHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({ file, content }),
    });
    if (res.status === 412) {
      sessionSaveState.set('conflict');
      sessionSaveDetail.set('Remote changed — reload file');
      return false;
    }
    if (!res.ok) {
      sessionSaveState.set('error');
      sessionSaveDetail.set((await res.text()) || `HTTP ${res.status}`);
      return false;
    }
    try {
      const data = await res.json();
      if (typeof data.revision === 'number') codingSessionRevision.set(data.revision);
    } catch {
      /* no body */
    }
    const revHdr = res.headers.get('x-veil-revision');
    if (revHdr) codingSessionRevision.set(Number(revHdr));
    sessionSaveState.set('saved');
    sessionSaveDetail.set(file);
    if (savedClearTimer) clearTimeout(savedClearTimer);
    savedClearTimer = setTimeout(() => {
      if (get(sessionSaveState) === 'saved') sessionSaveState.set('ready');
    }, 2500);
    return true;
  } catch (e) {
    sessionSaveState.set('error');
    sessionSaveDetail.set(e instanceof Error ? e.message : 'autosave failed');
    return false;
  }
}

/** Keep only palette entries from allowed layers (reaction mode: ['reaction']). */
export function filterPaletteByLayers(
  palette: PaletteEntry[],
  layers: string[],
): PaletteEntry[] {
  if (!layers.length) return palette;
  const allow = new Set(layers.map((l) => l.toLowerCase()));
  return palette.filter((c) => {
    if ((c.entry_type || 'construct') !== 'construct') return false;
    const layer = (c.layer || '').toLowerCase();
    return allow.has(layer);
  });
}

/**
 * Resolve IDE API base.
 * - Multi-project: `?project=name` → `/api/p/{name}`
 * - Single-project: `/api`
 */
export function ideApiBase(): string {
  const host = apiHost();
  const p = currentProjectParam();
  if (p) return `${host}/api/p/${encodeURIComponent(p)}`;
  return `${host}/api`;
}

export function hubApiBase(): string {
  return `${apiHost()}/api`;
}

export interface HubProject {
  name: string;
  path: string;
  is_git?: boolean;
  package_count?: number;
}

export interface HubSnapshot {
  multi: boolean;
  projects: HubProject[];
  projects_dir: string;
  config_path?: string;
}

export const hubSnapshot = writable<HubSnapshot | null>(null);

/** Detect multi vs single host; load project list (RTU-009). */
export async function fetchHubSnapshot(): Promise<HubSnapshot> {
  const empty: HubSnapshot = { multi: false, projects: [], projects_dir: '' };
  try {
    const res = await fetchWithTimeout(`${hubApiBase()}/projects`);
    if (!res.ok) {
      hubSnapshot.set(empty);
      return empty;
    }
    const data = await res.json();
    const projects: HubProject[] = data.projects || [];
    // Multi host: unscoped /api/ir is 404
    let multi = false;
    try {
      const ir = await fetchWithTimeout(`${hubApiBase()}/ir`);
      multi = ir.status === 404;
    } catch {
      multi = projects.length > 0;
    }
    const snap: HubSnapshot = {
      multi,
      projects,
      projects_dir: data.projects_dir || '',
      config_path: data.config_path,
    };
    hubSnapshot.set(snap);
    return snap;
  } catch {
    hubSnapshot.set(empty);
    return empty;
  }
}

export async function createHubProject(name: string): Promise<HubProject | null> {
  const res = await fetchWithTimeout(`${hubApiBase()}/projects`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
  if (!res.ok) return null;
  const info = await res.json();
  await fetchHubSnapshot();
  return info;
}

/** Navigate to a multi-project IDE URL (RTU-005). */
export function openProject(name: string) {
  if (typeof window === 'undefined') return;
  const u = new URL(window.location.href);
  u.searchParams.set('project', name);
  // Preserve api= override
  window.location.href = u.toString();
}

function api(path: string): string {
  return `${ideApiBase()}${path.startsWith('/') ? path : `/${path}`}`;
}

const API_URL = () => api('/ir');
const SOURCE_URL = () => api('/source');
const PALETTE_URL = () => api('/palette');
const PRESENTATION_URL = () => api('/presentation');
const CHECK_URL = () => api('/check');
const EDIT_URL = () => api('/edit');
const STUBS_URL = () => api('/stubs');
const FILES_URL = () => api('/files');
const SELECT_FILE_URL = () => api('/files/select');
const PROJECT_URL = () => api('/project');

/** Loaded file metadata from the server. */
export interface VeilFileInfo {
  index: number;
  name: string;
  path: string;
  editable: boolean;
  active: boolean;
  /** package | layer | stub (DSL-001) */
  kind?: 'package' | 'layer' | 'stub' | string;
  /** Adapt chain badge when package has `adapt` lines. */
  adapts?: string | null;
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

/** Durable session UX chip state. */
export type SessionSaveState =
  | 'idle'
  | 'ensuring'
  | 'ready'
  | 'saving'
  | 'saved'
  | 'conflict'
  | 'error';

export const sessionSaveState = writable<SessionSaveState>('idle');
export const sessionSaveDetail = writable<string | null>(null);
export const codingSessionRevision = writable<number | null>(null);
export const codingSessionMeta = writable<{
  session_id: string;
  slug: string;
  revision: number;
  draft_mode?: boolean;
  work_dir?: string;
} | null>(null);

function publishDiagsToHost(diags: Diagnostic[]) {
  // Lazy import path avoided — keep bridge tiny via dynamic import in browser only
  if (typeof window === 'undefined') return;
  void import('./agentPrompt').then(({ publishDiagnosticsSummary }) => {
    publishDiagnosticsSummary(diags.length, diags);
  });
}

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
      `${CHECK_URL()}?target=${encodeURIComponent(t || 'rust')}`
    );
    if (!res.ok && res.status !== 422) return null;
    const data: CheckResponse = await res.json();
    const diags = data.diagnostics ?? [];
    diagnostics.set(diags);
    publishDiagsToHost(diags);
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
  // Flow composer mode: always drill into the first fn/Flow regardless of
  // other siblings (layer-injected Bus, etc. don't block auto-drill).
  if (isFlowComposerMode() && flows.length >= 1) {
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

/**
 * Compare two IR graphs and return the set of node IDs that were added or changed.
 * A node is "changed" if its name, kind, fields, methods, or body differ.
 */
function diffNodes(prev: IrGraph, next: IrGraph): Set<number> {
  const changed = new Set<number>();
  const prevById = new Map(prev.nodes.map((n) => [n.id, n]));

  for (const node of next.nodes) {
    const old = prevById.get(node.id);
    if (!old) {
      // New node
      changed.add(node.id);
    } else if (nodeFingerprint(old) !== nodeFingerprint(node)) {
      // Modified node
      changed.add(node.id);
    }
  }
  return changed;
}

/** Quick fingerprint for IR node change detection (compare by value). */
function nodeFingerprint(n: IrNode): string {
  // Compare stable structural attributes: name, kind, subkind, properties,
  // annotations, and span (which changes when content before it changes).
  return `${n.name}|${n.kind}|${n.metadata?.subkind ?? ''}|${n.span.start}:${n.span.end}|${(n.metadata?.annotations ?? []).join(',')}|${(n.metadata?.properties ?? []).map(([k, v]) => `${k}=${v}`).join(',')}`;
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
  let prevGraphSnap: IrGraph | null = null;
  if (preserveNav) {
    prevParent = get(currentParent);
    prevCrumbs = get(breadcrumbs).slice();
    prevSel = get(selectedNodeId);
    prevGraphSnap = get(irGraph);
  }

  const modeHeaders = ideRequestHeaders();
  const withMode = (init?: RequestInit): RequestInit | undefined => {
    if (!Object.keys(modeHeaders).length) return init;
    return {
      ...init,
      headers: { ...(init?.headers as Record<string, string> | undefined), ...modeHeaders },
    };
  };
  const [irRes, srcRes, palRes, presRes, stubRes, filesRes, projRes] = await Promise.all([
    fetchWithTimeout(API_URL(), withMode()),
    fetchWithTimeout(SOURCE_URL(), withMode()),
    fetchWithTimeout(PALETTE_URL(), withMode()),
    fetchWithTimeout(PRESENTATION_URL(), withMode()).catch(() => null),
    fetchWithTimeout(STUBS_URL(), withMode()).catch(() => null),
    fetchWithTimeout(FILES_URL(), withMode()).catch(() => null),
    fetchWithTimeout(PROJECT_URL(), withMode()).catch(() => null),
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

  // Detect changed nodes for flash animation (only on preserveNav / agent edits).
  if (preserveNav) {
    let prevGraph: IrGraph | null = null;
    const unsub = irGraph.subscribe((g) => { prevGraph = g; });
    unsub();
    if (prevGraph) {
      const changed = diffNodes(prevGraph, data);
      changedNodeIds.set(changed);
      // Auto-clear flash after animation duration.
      setTimeout(() => changedNodeIds.set(new Set()), 1200);
    }
  }

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
    let palette: PaletteEntry[] = await palRes.json();
    // Embed shell: only constructs from configured layers (reaction → ['reaction']).
    const shell = embedShellConfig();
    if (shell.paletteLayers.length) {
      palette = filterPaletteByLayers(palette, shell.paletteLayers);
    }
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
  void fetchWithTimeout(`${ideApiBase()}/generated`)
    .then(async (r) => {
      if (gen !== loadGeneration || !r.ok) return;
      generatedCode.set(await r.json());
    })
    .catch(() => {});

  if (preserveNav) {
    // Restore drill-down parent (by id, then by name/kind from previous graph)
    let restoredParent = prevParent;
    if (prevParent != null && !data.nodes.some((n) => n.id === prevParent)) {
      const oldP = prevGraphSnap?.nodes.find((n) => n.id === prevParent);
      const match = oldP
        ? data.nodes.find((n) => n.name === oldP.name && n.kind === oldP.kind)
        : undefined;
      restoredParent = match?.id ?? null;
    }
    if (restoredParent != null && data.nodes.some((n) => n.id === restoredParent)) {
      currentParent.set(restoredParent);
      const crumbs = prevCrumbs
        .map((c) => {
          if (c.id == null) return c;
          if (data.nodes.some((n) => n.id === c.id)) return c;
          const oldC = prevGraphSnap?.nodes.find((n) => n.id === c.id);
          const hit = oldC
            ? data.nodes.find((n) => n.name === oldC.name && n.kind === oldC.kind)
            : undefined;
          return hit ? { id: hit.id, name: hit.name } : null;
        })
        .filter((c): c is { id: number | null; name: string } => c != null);
      breadcrumbs.set(
        crumbs.length
          ? crumbs
          : [{ id: restoredParent, name: data.nodes.find((n) => n.id === restoredParent)?.name ?? '' }]
      );
    } else {
      applyRootNavigation(data);
    }

    // Restore selection for detail/main panel (id, then name+kind)
    let restoredSel: string | null = null;
    if (prevSel) {
      if (data.nodes.some((n) => String(n.id) === prevSel)) {
        restoredSel = prevSel;
      } else {
        const oldN = prevGraphSnap?.nodes.find((n) => String(n.id) === prevSel);
        const hit = oldN
          ? data.nodes.find((n) => n.name === oldN.name && n.kind === oldN.kind)
          : undefined;
        if (hit) restoredSel = String(hit.id);
      }
    }
    selectedNodeId.set(restoredSel);
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
    const hub = await fetchHubSnapshot();
    // RTU-009: multi host without ?project= → leave loading false, page shows picker
    if (hub.multi && !currentProjectParam()) {
      loading.set(false);
      return;
    }
    await loadActiveFile(gen);
  } catch (e) {
    if (gen === loadGeneration) {
      const msg =
        e instanceof Error
          ? e.name === 'AbortError'
            ? `Timed out talking to API at ${ideApiBase()} — start veil-runtime (:8080) or veil serve --multi (:3001)?`
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
            ? `Timed out talking to API at ${ideApiBase()}`
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

/** Set of node IDs that changed in the last refresh (for flash animation). */
export const changedNodeIds = writable<Set<number>>(new Set());
/** When true, the agent is actively making edits (revisions arriving). */
export const agentActive = writable(false);
let agentActiveTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Subscribe to `GET /api/events` so agent mid-turn writes update the badge
 * without waiting for the HTTP turn response.
 */
export function startRevisionWatch(): () => void {
  stopRevisionWatch();
  try {
    sse = new EventSource(`${ideApiBase()}/events`);
  } catch {
    return () => {};
  }
  const onRevision = (ev: MessageEvent) => {
    try {
      const data = JSON.parse(String(ev.data || '{}')) as {
        revision?: number;
        reason?: string;
        active_file?: string;
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

      // Signal agent activity (auto-clears after 2s of silence).
      agentActive.set(true);
      if (agentActiveTimer) clearTimeout(agentActiveTimer);
      agentActiveTimer = setTimeout(() => agentActive.set(false), 2000);

      // Debounce bursty multi-tool writes
      if (sseRefreshTimer) clearTimeout(sseRefreshTimer);
      sseRefreshTimer = setTimeout(() => {
        if (data.reason === 'select_file') {
          // Agent switched files — full reload (new file, reset navigation).
          void fetchIr();
        } else {
          void refreshAfterEdit();
        }
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
  if (agentActiveTimer) {
    clearTimeout(agentActiveTimer);
    agentActiveTimer = null;
  }
  agentActive.set(false);
  if (sse) {
    sse.close();
    sse = null;
  }
  lastSseRevision = null;
}

/**
 * Build breadcrumb chain from IR parents: Solution → Module → … → node.
 * Skips pure Group buckets so "Back" lands on a useful host (module/service).
 */
export function breadcrumbChainFor(graph: IrGraph, node: IrNode): { id: number; name: string }[] {
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const chain: { id: number; name: string }[] = [];
  let cur: IrNode | undefined = node;
  const seen = new Set<number>();
  while (cur && !seen.has(cur.id)) {
    seen.add(cur.id);
    // Keep Groups in the chain so navigateTo can restore exact parent; labels stay useful.
    chain.unshift({ id: cur.id, name: cur.name });
    const pid = cur.metadata.parent;
    cur = pid != null ? byId.get(pid) : undefined;
  }
  return chain;
}

/** Drill into a node (e.g. DomainService → flow graph). Rebuilds full ancestor crumbs. */
export function drillDown(node: IrNode) {
  const graph = get(irGraph);
  if (graph) {
    breadcrumbs.set(breadcrumbChainFor(graph, node));
  } else {
    breadcrumbs.update((bc) => [...bc, { id: node.id, name: node.name }]);
  }
  currentParent.set(node.id);
  selectedNodeId.set(null);
}

/** Navigate to an ancestor (breadcrumb click). */
export function navigateTo(id: number | null) {
  const graph = get(irGraph);
  if (id == null) {
    const sol = graph?.nodes.find((n) => n.kind === 'Solution');
    if (sol) {
      currentParent.set(sol.id);
      breadcrumbs.set([{ id: sol.id, name: sol.name }]);
    }
    return;
  }
  currentParent.set(id);
  if (graph) {
    const node = graph.nodes.find((n) => n.id === id);
    if (node) {
      breadcrumbs.set(breadcrumbChainFor(graph, node));
      return;
    }
  }
  breadcrumbs.update((bc) => {
    const idx = bc.findIndex((b) => b.id === id);
    return idx >= 0 ? bc.slice(0, idx + 1) : bc;
  });
}

/** One level up from the current host (simple parent). */
export function navigateUp(): boolean {
  const bc = get(breadcrumbs);
  if (bc.length >= 2) {
    const parent = bc[bc.length - 2];
    navigateTo(parent.id);
    return true;
  }
  const graph = get(irGraph);
  const curId = get(currentParent);
  if (graph && curId != null) {
    const cur = graph.nodes.find((n) => n.id === curId);
    const pid = cur?.metadata.parent;
    if (pid != null) {
      navigateTo(pid);
      return true;
    }
    const sol = graph.nodes.find((n) => n.kind === 'Solution');
    if (sol) {
      navigateTo(sol.id);
      return true;
    }
  }
  return false;
}

/**
 * Leave a flow graph for the nearest useful tree host.
 *
 * Skips `Group` buckets (domain/application/…) so Back from a DomainService
 * returns to the Module (full domain model outline), not a service-only group.
 */
export function navigateUpFromFlow(): boolean {
  const graph = get(irGraph);
  const curId = get(currentParent);
  if (!graph || curId == null) return navigateUp();

  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  let pid = byId.get(curId)?.metadata.parent ?? null;
  while (pid != null) {
    const p = byId.get(pid);
    if (!p) break;
    if (p.kind === 'Group') {
      pid = p.metadata.parent;
      continue;
    }
    // Module / Solution / other structural hosts get a full outline again
    navigateTo(p.id);
    return true;
  }
  return navigateUp();
}

/** Switch to a different loaded file by index. Re-fetches IR + all panels (UX-011). */
export async function selectFile(index: number) {
  const gen = ++loadGeneration;
  loading.set(true);
  error.set(null);
  try {
    const res = await fetchWithTimeout(SELECT_FILE_URL(), {
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
            ? `Timed out selecting file (API ${ideApiBase()})`
            : e.message
          : 'Failed to switch file';
      error.set(msg);
    }
  } finally {
    if (gen === loadGeneration) loading.set(false);
  }
}

export interface CreateFileResult {
  ok: boolean;
  index: number;
  name: string;
  path: string;
  kind: string;
  files?: VeilFileInfo[];
}

/**
 * Create a package (`.veil`) or layer (`.layer`) in the active project,
 * register it in the serve set, and switch the IDE to it.
 */
export async function createFile(opts: {
  name: string;
  kind?: 'package' | 'layer';
  content?: string;
}): Promise<CreateFileResult | null> {
  const name = opts.name.trim();
  if (!name) return null;
  const gen = ++loadGeneration;
  loading.set(true);
  error.set(null);
  try {
    const res = await fetchWithTimeout(FILES_URL(), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name,
        kind: opts.kind ?? 'package',
        content: opts.content,
      }),
    });
    if (!res.ok) {
      const body = await res.text().catch(() => '');
      const detail = body.trim().slice(0, 400);
      throw new Error(
        detail
          ? `Failed to create file: HTTP ${res.status}: ${detail}`
          : `Failed to create file: HTTP ${res.status}`
      );
    }
    const data: CreateFileResult = await res.json();
    if (data.files && Array.isArray(data.files)) {
      availableFiles.set(data.files);
      const active = data.files.find((f) => f.active) ?? data.files[data.index];
      if (active) {
        activeFileName.set(active.name);
        activeFileKind.set(active.kind || data.kind || 'package');
      }
    }
    if (gen !== loadGeneration) return data;
    await loadActiveFile(gen);
    return data;
  } catch (e) {
    if (gen === loadGeneration) {
      const msg =
        e instanceof Error
          ? e.name === 'AbortError'
            ? `Timed out creating file (API ${ideApiBase()})`
            : e.message
          : 'Failed to create file';
      error.set(msg);
    }
    return null;
  } finally {
    if (gen === loadGeneration) loading.set(false);
  }
}

/**
 * Select a graph node from a diagnostic and open the diagnostics panel.
 * Prefers `node_id`; falls back to matching `node_name`.
 * Does not change outline host (package tree stays put).
 */
export function focusDiagnostic(diag: Diagnostic) {
  diagnosticsPanelOpen.set(true);

  const graph = get(irGraph);
  if (!graph) return;

  let node: IrNode | undefined;
  if (diag.node_id != null) {
    node = graph.nodes.find((n) => n.id === diag.node_id);
  }
  if (!node && diag.node_name) {
    node = graph.nodes.find((n) => n.name === diag.node_name);
  }
  if (!node) return;
  selectedNodeId.set(String(node.id));
}

/** Get children of a given parent node */
export function getChildren(graph: IrGraph, parentId: number | null): IrNode[] {
  if (parentId === null) {
    // Package root: children of the Solution node (not the Solution itself).
    const sol = graph.nodes.find((n) => n.kind === 'Solution' && n.metadata.parent == null)
      ?? graph.nodes.find((n) => n.kind === 'Solution');
    if (sol) {
      return graph.nodes.filter((n) => n.metadata.parent === sol.id);
    }
    return graph.nodes.filter((n) => n.metadata.parent === null && n.kind !== 'Solution');
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

/** Edit annotation metadata (UX-030). */
export interface EditAnnotation {
  intent?: string;
  category?: 'structure' | 'behavior' | 'constraint' | 'integration' | 'cosmetic' | 'docs';
  criticality?: 'critical' | 'high' | 'normal' | 'low';
}

/**
 * Persist a batch of structured edits to the server. The server applies them
 * to the AST, re-serializes + validates, writes the .veil file, and returns
 * fresh source / IR / generated code, which we push into the stores so every
 * panel (graph, source, code preview) updates live.
 *
 * @param annotations Optional per-edit metadata (same length as edits; null entries = no annotation).
 * Returns true on success; on failure sets `saveError` and leaves state intact.
 */
export async function saveEdits(
  edits: EditOp[],
  annotations?: (EditAnnotation | null)[],
): Promise<boolean> {
  if (edits.length === 0) return true;
  if (!getCodingSessionId()) {
    await ensureCodingSession();
  }
  saving.set(true);
  saveError.set(null);
  sessionSaveState.set('saving');
  try {
    const body: { edits: EditOp[]; annotations?: (EditAnnotation | null)[] } = { edits };
    if (annotations && annotations.some((a) => a != null)) {
      body.annotations = annotations;
    }
    const res = await fetch(EDIT_URL(), {
      method: 'POST',
      headers: ideRequestHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify(body),
    });
    if (res.status === 412) {
      const msg = await res.text();
      saveError.set(msg || 'Conflict — remote changed');
      sessionSaveState.set('conflict');
      sessionSaveDetail.set(msg || 'etag conflict');
      return false;
    }
    if (!res.ok) {
      const msg = await res.text();
      saveError.set(msg || `HTTP ${res.status}`);
      sessionSaveState.set('error');
      sessionSaveDetail.set(msg || `HTTP ${res.status}`);
      return false;
    }
    const revHdr = res.headers.get('x-veil-revision');
    if (revHdr) codingSessionRevision.set(Number(revHdr));
    const data: {
      source: string;
      ir: IrGraph;
      generated: Record<string, string>;
      diagnostics?: Diagnostic[];
      resolved_annotations?: EditAnnotation[];
    } = await res.json();
    irGraph.set(data.ir);
    veilSource.set(data.source);
    generatedCode.set(data.generated);
    if (data.diagnostics) {
      diagnostics.set(data.diagnostics);
      publishDiagsToHost(data.diagnostics);
    } else {
      await fetchCheck();
    }
    sessionSaveState.set('saved');
    sessionSaveDetail.set(get(activeFileName) || 'saved');
    if (savedClearTimer) clearTimeout(savedClearTimer);
    savedClearTimer = setTimeout(() => {
      if (get(sessionSaveState) === 'saved') sessionSaveState.set('ready');
    }, 2500);
    return true;
  } catch (e) {
    saveError.set(e instanceof Error ? e.message : 'Save failed');
    sessionSaveState.set('error');
    sessionSaveDetail.set(e instanceof Error ? e.message : 'Save failed');
    return false;
  } finally {
    saving.set(false);
  }
}
