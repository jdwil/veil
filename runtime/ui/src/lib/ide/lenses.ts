/**
 * Layer-declared review lenses (LAY-009 / UX-022).
 * Criticality comes from presentation IR + diagnostics — not keyword lists.
 */

import type { IrGraph, IrNode } from './types';
import type { PresentationModel } from './presentation';
import type { Diagnostic } from './store';

const ESCAPE_CODES = new Set([
  'escape_raw',
  'escape_empty_adapter',
  'escape_external_call',
  'escape_json_boundary',
  'escape_hatch',
]);

function constructName(n: IrNode): string {
  return n.metadata.subkind ?? n.kind;
}

/** Lenses declared on this construct type in the presentation model. */
export function lensesForNode(
  node: IrNode,
  presentation: PresentationModel | null
): string[] {
  if (!presentation) return [];
  const name = constructName(node);
  return presentation.constructs[name]?.lenses ?? [];
}

export function nodeHasLens(
  node: IrNode,
  lens: string,
  presentation: PresentationModel | null
): boolean {
  return lensesForNode(node, presentation).includes(lens);
}

/** Escape-hatch / critical diagnostics attached to a node. */
export function nodeHasCriticalDiagnostic(
  node: IrNode,
  diags: Diagnostic[]
): boolean {
  return diags.some((d) => {
    const onNode =
      d.node_id === node.id ||
      (d.node_name != null && d.node_name === node.name);
    if (!onNode) return false;
    const code = (d.code ?? '').toLowerCase();
    if (ESCAPE_CODES.has(code) || code.startsWith('escape_')) return true;
    // High-severity errors also count as review-critical
    if (d.severity === 'Error' || d.severity === 'error') return true;
    return false;
  });
}

/**
 * Layer presentation lens `critical` — architecturally important for review.
 * **Not** a compile/check failure. UI uses soft blue + hourglass (not warning colors).
 */
export function nodeHasReviewLens(
  node: IrNode,
  presentation: PresentationModel | null
): boolean {
  return nodeHasLens(node, 'critical', presentation);
}

/**
 * Real health problem on this construct (error / escape hatch).
 * UI keeps amber/red warning styling.
 */
export function nodeHasHealthIssue(
  node: IrNode,
  diags: Diagnostic[]
): boolean {
  return nodeHasCriticalDiagnostic(node, diags);
}

/**
 * Union for "Critical / review focus" filter (lens OR health).
 * Prefer {@link nodeHasReviewLens} / {@link nodeHasHealthIssue} for presentation.
 */
export function isCriticalNode(
  node: IrNode,
  presentation: PresentationModel | null,
  diags: Diagnostic[]
): boolean {
  if (nodeHasReviewLens(node, presentation)) return true;
  if (nodeHasHealthIssue(node, diags)) return true;
  // Layer-provided infrastructure is not "critical" for review focus
  if (node.metadata.annotations.includes('layer-provided')) return false;
  return false;
}

export function countCritical(
  graph: IrGraph,
  presentation: PresentationModel | null,
  diags: Diagnostic[]
): number {
  return graph.nodes.filter(
    (n) =>
      n.kind !== 'Solution' &&
      !n.metadata.annotations.includes('layer-provided') &&
      isCriticalNode(n, presentation, diags)
  ).length;
}

/** Collect all unique lens ids declared across all constructs in the presentation model. */
export function collectAllLenses(presentation: PresentationModel | null): string[] {
  if (!presentation) return [];
  const lenses = new Set<string>();
  for (const dto of Object.values(presentation.constructs)) {
    for (const l of dto.lenses ?? []) {
      lenses.add(l);
    }
  }
  // Stable order: critical first, integration second, rest alpha
  const ordered = [...lenses].sort((a, b) => {
    if (a === 'critical') return -1;
    if (b === 'critical') return 1;
    if (a === 'integration') return -1;
    if (b === 'integration') return 1;
    return a.localeCompare(b);
  });
  return ordered;
}

/** Count nodes matching ANY of the given lenses OR having critical diagnostics. */
export function countByLenses(
  graph: IrGraph,
  presentation: PresentationModel | null,
  diags: Diagnostic[],
  lenses: Set<string>
): number {
  if (lenses.size === 0) return 0;
  return graph.nodes.filter((n) => {
    if (n.kind === 'Solution') return false;
    if (n.metadata.annotations.includes('layer-provided')) return false;
    const nodeLenses = lensesForNode(n, presentation);
    // Node matches if it has any of the active lenses
    if (nodeLenses.some(l => lenses.has(l))) return true;
    // If 'critical' lens is active, also include escape-hatch/error nodes
    if (lenses.has('critical') && nodeHasCriticalDiagnostic(n, diags)) return true;
    return false;
  }).length;
}

/** Check if a node matches any of the active lenses. */
export function nodeMatchesLenses(
  node: IrNode,
  presentation: PresentationModel | null,
  diags: Diagnostic[],
  lenses: Set<string>
): boolean {
  if (lenses.size === 0) return true; // No filter = show all
  const nodeLenses = lensesForNode(node, presentation);
  if (nodeLenses.some(l => lenses.has(l))) return true;
  if (lenses.has('critical') && nodeHasCriticalDiagnostic(node, diags)) return true;
  return false;
}
