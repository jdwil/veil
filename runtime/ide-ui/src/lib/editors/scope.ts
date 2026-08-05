/**
 * scope.ts — Resolve variables available at a given node position in the flow graph.
 *
 * Used by the RuleBuilder to provide LHS field options for Decision nodes.
 * Walks the IR graph to find:
 *   1. Function signature parameters (struct members expanded as dot-paths)
 *   2. Upstream node bindings (result_binding, binding, item_binding, index_binding)
 */

import type { IrGraph, IrNode, IrEdge } from '$lib/types';

export interface ScopeField {
  path: string;
  type: string;
  label?: string;
}

/**
 * Resolve all fields/variables available at the given node position.
 *
 * Strategy:
 *   1. Find the parent Flow/fn node → parse its params for input types
 *   2. If a param type matches a known struct in the graph, expand its fields
 *   3. Walk upstream nodes (predecessors via SequenceFlow edges) and collect bindings
 *   4. Check for enclosing Loop nodes (item_binding, index_binding)
 */
export function resolveFieldsInScope(graph: IrGraph, nodeId: number): ScopeField[] {
  const fields: ScopeField[] = [];
  const node = graph.nodes.find(n => n.id === nodeId);
  if (!node) return fields;

  // 1. Find parent Flow node
  const flowNode = findParentFlow(graph, node);
  if (flowNode) {
    // Parse function params from properties: "params" → "(event: MyEvent)" or "(ctx: Json)"
    const paramsStr = flowNode.metadata.properties.find(([k]) => k === 'params')?.[1] ?? '';
    const parsedParams = parseParams(paramsStr);

    for (const param of parsedParams) {
      // Try to expand struct types
      const structFields = resolveStructFields(graph, param.type);
      if (structFields.length > 0) {
        for (const sf of structFields) {
          fields.push({
            path: `${param.name}.${sf.name}`,
            type: sf.type,
            label: `${param.name}.${sf.name}`,
          });
        }
      } else {
        // Scalar or unknown type — expose as single variable
        fields.push({
          path: param.name,
          type: param.type,
          label: param.name,
        });
      }
    }
  }

  // 2. Walk upstream nodes via SequenceFlow edges and collect bindings
  const upstream = collectUpstreamNodes(graph, nodeId);
  for (const upNode of upstream) {
    const props = upNode.metadata.properties;
    const subkind = upNode.metadata.subkind ?? '';

    // result_binding from Query / RepositoryAccess / Relay nodes
    const resultBinding = props.find(([k]) => k === 'result_binding')?.[1];
    if (resultBinding) {
      // Try to infer type from the method return, otherwise generic Json
      const inferredType = inferBindingType(graph, upNode) ?? 'Json';
      fields.push({
        path: resultBinding,
        type: inferredType,
        label: `${resultBinding} (from ${upNode.name || subkind})`,
      });
    }

    // binding from Assign nodes
    const assignBinding = props.find(([k]) => k === 'binding')?.[1];
    if (assignBinding && subkind === 'assign') {
      fields.push({
        path: assignBinding,
        type: 'Json',
        label: `${assignBinding} (assign)`,
      });
    }
  }

  // 3. Check for enclosing Loop nodes (parent chain)
  const enclosingLoops = findEnclosingLoops(graph, node);
  for (const loop of enclosingLoops) {
    const itemBinding = loop.metadata.properties.find(([k]) => k === 'item_binding')?.[1];
    const indexBinding = loop.metadata.properties.find(([k]) => k === 'index_binding')?.[1];
    if (itemBinding) {
      fields.push({
        path: itemBinding,
        type: 'Json',
        label: `${itemBinding} (loop item)`,
      });
    }
    if (indexBinding) {
      fields.push({
        path: indexBinding,
        type: 'Int',
        label: `${indexBinding} (loop index)`,
      });
    }
  }

  return fields;
}

// ─── Internal helpers ──────────────────────────────────────────────────────

function findParentFlow(graph: IrGraph, node: IrNode): IrNode | null {
  let current: IrNode | undefined = node;
  while (current) {
    if (current.kind === 'Flow') return current;
    const parentId: number | null = current.metadata.parent;
    if (parentId === null) return null;
    current = graph.nodes.find(n => n.id === parentId);
  }
  return null;
}

interface ParsedParam {
  name: string;
  type: string;
}

function parseParams(paramsStr: string): ParsedParam[] {
  // Input: "(event: MyEvent, ctx: Json)" or "event: MyEvent, ctx: Json"
  const clean = paramsStr.replace(/^\(/, '').replace(/\)$/, '').trim();
  if (!clean) return [];

  return clean.split(',').map(part => {
    const trimmed = part.trim();
    const colonIdx = trimmed.indexOf(':');
    if (colonIdx < 0) return { name: trimmed, type: 'Json' };
    return {
      name: trimmed.slice(0, colonIdx).trim(),
      type: trimmed.slice(colonIdx + 1).trim(),
    };
  }).filter(p => p.name);
}

/**
 * If a type name corresponds to a TypeDef node in the graph, return its fields.
 */
function resolveStructFields(graph: IrGraph, typeName: string): { name: string; type: string }[] {
  // Look for a TypeDef node with this name
  const typeDef = graph.nodes.find(
    n => n.kind === 'TypeDef' && n.name === typeName
  );
  if (!typeDef) return [];

  // TypeDef fields are child Field nodes, or stored in a "fields" property
  const fieldChildren = graph.nodes.filter(
    n => n.metadata.parent === typeDef.id && n.kind === 'Field'
  );
  if (fieldChildren.length > 0) {
    return fieldChildren.map(f => {
      const type = f.metadata.properties.find(([k]) => k === 'type')?.[1] ?? 'Str';
      return { name: f.name, type };
    });
  }

  // Fallback: parse "fields" property if present (format: "name: Type, name2: Type2")
  const fieldsProp = typeDef.metadata.properties.find(([k]) => k === 'fields')?.[1] ?? '';
  if (fieldsProp) {
    return fieldsProp.split(',').map(f => {
      const parts = f.trim().split(':');
      return {
        name: parts[0]?.trim() ?? '',
        type: parts[1]?.trim() ?? 'Str',
      };
    }).filter(f => f.name);
  }

  return [];
}

/**
 * Collect all nodes that are upstream of the given node via SequenceFlow edges.
 * BFS traversal backwards through the graph.
 */
function collectUpstreamNodes(graph: IrGraph, nodeId: number): IrNode[] {
  const visited = new Set<number>();
  const queue: number[] = [];
  const result: IrNode[] = [];

  // Find all edges pointing TO this node (predecessors)
  const incomingEdges = graph.edges.filter(
    e => e.to === nodeId && e.kind === 'SequenceFlow'
  );
  for (const edge of incomingEdges) {
    queue.push(edge.from);
  }

  while (queue.length > 0) {
    const current = queue.shift()!;
    if (visited.has(current)) continue;
    visited.add(current);

    const node = graph.nodes.find(n => n.id === current);
    if (node) {
      result.push(node);
      // Continue traversing upstream
      const moreIncoming = graph.edges.filter(
        e => e.to === current && e.kind === 'SequenceFlow'
      );
      for (const edge of moreIncoming) {
        if (!visited.has(edge.from)) {
          queue.push(edge.from);
        }
      }
    }
  }

  return result;
}

/**
 * Find Loop nodes that are ancestors (parents) of the given node.
 */
function findEnclosingLoops(graph: IrGraph, node: IrNode): IrNode[] {
  const loops: IrNode[] = [];
  let current: IrNode | undefined = node;
  while (current) {
    const parentId: number | null = current.metadata.parent;
    if (parentId === null) break;
    const parent: IrNode | undefined = graph.nodes.find(n => n.id === parentId);
    if (parent && parent.metadata.subkind === 'loop') {
      loops.push(parent);
    }
    current = parent;
  }
  return loops;
}

/**
 * Try to infer the return type of a binding from its source node.
 * For RepositoryAccess/Query nodes, look at the method's return type.
 */
function inferBindingType(_graph: IrGraph, _node: IrNode): string | null {
  // Future: could resolve via callable metadata. For now, default to Json.
  return null;
}
