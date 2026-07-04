// VEIL IR Types — mirrors the Rust IR graph model.
//
// The viewer contains ZERO domain knowledge. NODE_STYLES below covers only
// core language shapes; all layer vocabulary (icons, colors, labels for
// aggregates, ports, sagas, or any future layer's constructs) arrives at
// runtime via /api/palette and is registered with setPaletteStyles().

export interface IrGraph {
  nodes: IrNode[];
  edges: IrEdge[];
  next_id: number;
}

export interface IrNode {
  id: number;
  kind: NodeKind;
  name: string;
  span: { start: number; end: number };
  metadata: NodeMetadata;
}

export interface NodeMetadata {
  parent: number | null;
  annotations: string[];
  properties: [string, string][];
  subkind: string | null;
}

export type NodeKind =
  | 'Solution'
  | 'Module'
  | 'Group'
  | 'Inputs'
  | 'TypeDef'
  | 'Interface'
  | 'InterfaceMethod'
  | 'Implementation'
  | 'Flow'
  | 'Step'
  | 'ParallelGateway'
  | 'ErrorBoundary'
  | 'Action'
  | 'MatchDecision'
  | 'MatchArm';

export interface IrEdge {
  from: number;
  to: number;
  kind: EdgeKind;
}

export type EdgeKind =
  | 'Contains'
  | 'SequenceFlow'
  | 'Calls'
  | 'Emits'
  | 'Implements'
  | 'References';

export interface NodeStyle {
  color: string;
  icon: string;
  label: string;
}

/** A palette entry served by /api/palette — parsed from .layer files. */
export interface PaletteEntry {
  name: string;
  keyword: string;
  kind: string;
  shape: string;
  icon: string;
  color: string;
  label: string;
  group: string;
  allowed_in: string;
  layer: string;
  entry_type: 'construct' | 'statement';
}

// Visual config per node kind — CORE SHAPES ONLY.
export const NODE_STYLES: Record<NodeKind, NodeStyle> = {
  Solution: { color: '#6366f1', icon: '🏗️', label: 'Solution' },
  Module: { color: '#8b5cf6', icon: '📦', label: 'Module' },
  Group: { color: '#475569', icon: '📂', label: 'Group' },
  Inputs: { color: '#22c55e', icon: '📥', label: 'Inputs' },
  TypeDef: { color: '#14b8a6', icon: '📋', label: 'Type' },
  Interface: { color: '#10b981', icon: '🔌', label: 'Interface' },
  InterfaceMethod: { color: '#34d399', icon: '⚙️', label: 'Method' },
  Implementation: { color: '#a855f7', icon: '🔗', label: 'Implementation' },
  Flow: { color: '#f97316', icon: '🌊', label: 'Flow' },
  Step: { color: '#64748b', icon: '▶️', label: 'Step' },
  ParallelGateway: { color: '#eab308', icon: '⑃', label: 'Parallel' },
  ErrorBoundary: { color: '#ef4444', icon: '🛡️', label: 'Error Boundary' },
  Action: { color: '#10b981', icon: '▸', label: 'Action' },
  MatchDecision: { color: '#8b5cf6', icon: '◆', label: 'Match' },
  MatchArm: { color: '#64748b', icon: '→', label: 'Arm' },
};

// Core statement styles (call/assign are language-level, not layer-level).
const CORE_ACTION_STYLES: Record<string, NodeStyle> = {
  call: { color: '#10b981', icon: '📞', label: 'Call' },
  assign: { color: '#6366f1', icon: '←', label: 'Assign' },
};

// Runtime style registry, populated from /api/palette. Keyed by both the
// construct name (subkind, e.g. "Aggregate") and keyword (e.g. "agg" or
// statement keywords like "dispatch").
let paletteStyles: Record<string, NodeStyle> = {};

/** Register layer visuals fetched from /api/palette. */
export function setPaletteStyles(entries: PaletteEntry[]): void {
  const styles: Record<string, NodeStyle> = {};
  for (const e of entries) {
    if (!e.icon && !e.color) continue;
    const style: NodeStyle = {
      color: e.color || '#64748b',
      icon: e.icon || '•',
      label: e.label || e.name,
    };
    styles[e.name] = style;
    if (e.keyword && e.keyword !== e.name) styles[e.keyword] = style;
  }
  paletteStyles = styles;
}

/**
 * Get the display style for a node. Precedence:
 * layer-defined subkind style → core action style → core shape style.
 */
export function getNodeStyle(kind: NodeKind, subkind?: string | null): NodeStyle {
  if (subkind) {
    if (paletteStyles[subkind]) return paletteStyles[subkind];
    if (CORE_ACTION_STYLES[subkind]) return CORE_ACTION_STYLES[subkind];
  }
  return NODE_STYLES[kind] ?? { color: '#64748b', icon: '•', label: kind };
}
