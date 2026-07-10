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
  | 'Field'
  | 'Return'
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

/** A layer-declared annotation available on a construct (from /api/palette). */
export interface AnnotationSpec {
  name: string;
  desc: string;
  params: string[];
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
  annotations?: AnnotationSpec[];
  /** Default group for create/placement (layer `dg`). */
  dg?: string;
  expected_groups?: string[];
}

// Visual config per node kind — CORE SHAPES ONLY.
export const NODE_STYLES: Record<NodeKind, NodeStyle> = {
  Solution: { color: '#737373', icon: '🏗️', label: 'Solution' },
  Module: { color: '#737373', icon: '📦', label: 'Module' },
  Group: { color: '#525252', icon: '📂', label: 'Group' },
  Inputs: { color: '#a3a3a3', icon: '📥', label: 'Inputs' },
  Field: { icon: '📋', color: '#6b7280' },
  Return: { color: '#a3a3a3', icon: '📤', label: 'Return' },
  TypeDef: { color: '#737373', icon: '📋', label: 'Type' },
  Interface: { color: '#737373', icon: '🔌', label: 'Interface' },
  InterfaceMethod: { color: '#a3a3a3', icon: '⚙️', label: 'Method' },
  Implementation: { color: '#737373', icon: '🔗', label: 'Implementation' },
  Flow: { color: '#a3a3a3', icon: '🌊', label: 'Flow' },
  Step: { color: '#737373', icon: '▶️', label: 'Step' },
  ParallelGateway: { color: '#a3a3a3', icon: '⑃', label: 'Parallel' },
  ErrorBoundary: { color: '#ef4444', icon: '🛡️', label: 'Error Boundary' },
  Action: { color: '#737373', icon: '▸', label: 'Action' },
  MatchDecision: { color: '#a3a3a3', icon: '◆', label: 'Match' },
  MatchArm: { color: '#737373', icon: '→', label: 'Arm' },
};

// Core statement styles (call/assign are language-level, not layer-level).
const CORE_ACTION_STYLES: Record<string, NodeStyle> = {
  call: { color: '#737373', icon: '📞', label: 'Call' },
  assign: { color: '#a3a3a3', icon: '←', label: 'Assign' },
};

// Runtime style registry, populated from /api/palette. Keyed by both the
// construct name (subkind, e.g. "Aggregate") and keyword (e.g. "agg" or
// statement keywords like "dispatch").
let paletteStyles: Record<string, NodeStyle> = {};

// Layer-declared annotation definitions, keyed by construct name AND keyword,
// so the property editor can offer them without any hardcoded DDD vocabulary.
let paletteAnnotations: Record<string, AnnotationSpec[]> = {};

/** Register layer visuals + annotations fetched from /api/palette. */
export function setPaletteStyles(entries: PaletteEntry[]): void {
  const styles: Record<string, NodeStyle> = {};
  const annotations: Record<string, AnnotationSpec[]> = {};
  for (const e of entries) {
    if (e.icon || e.color) {
      const style: NodeStyle = {
        color: e.color || '#737373',
        icon: e.icon || '•',
        label: e.label || e.name,
      };
      styles[e.name] = style;
      if (e.keyword && e.keyword !== e.name) styles[e.keyword] = style;
    }
    if (e.annotations && e.annotations.length > 0) {
      annotations[e.name] = e.annotations;
      if (e.keyword && e.keyword !== e.name) annotations[e.keyword] = e.annotations;
    }
  }
  paletteStyles = styles;
  paletteAnnotations = annotations;
}

/** Annotation definitions available for a construct subkind (layer-driven). */
export function getAnnotationDefs(subkind?: string | null): AnnotationSpec[] {
  if (subkind && paletteAnnotations[subkind]) return paletteAnnotations[subkind];
  return [];
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
  return NODE_STYLES[kind] ?? { color: '#737373', icon: '•', label: kind };
}
