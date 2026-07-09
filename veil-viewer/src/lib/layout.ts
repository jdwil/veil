import ELK from 'elkjs/lib/elk.bundled.js';
import type { Node, Edge } from '@xyflow/svelte';

const elk = new ELK();

const DEFAULT_NODE_WIDTH = 260;
const DEFAULT_NODE_HEIGHT = 80;

/**
 * Lay out nodes using ELK.js for flow-type views (services, steps, methods).
 * Replaces dagre for directed graph layouts.
 */
export async function layoutNodes(
  nodes: Node[],
  edges: Edge[],
  direction: 'TB' | 'LR' = 'TB'
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  const elkDirection = direction === 'LR' ? 'RIGHT' : 'DOWN';

  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': elkDirection,
      'elk.spacing.nodeNode': '40',
      'elk.layered.spacing.nodeNodeBetweenLayers': '100',
    },
    children: nodes.map((node) => ({
      id: node.id,
      width: DEFAULT_NODE_WIDTH,
      height: DEFAULT_NODE_HEIGHT,
    })),
    edges: edges.map((edge, i) => ({
      id: `e${i}`,
      sources: [edge.source],
      targets: [edge.target],
    })),
  };

  try {
    const layout = await elk.layout(elkGraph);

    const positionMap = new Map<string, { x: number; y: number }>();
    for (const child of layout.children ?? []) {
      positionMap.set(child.id, { x: child.x ?? 0, y: child.y ?? 0 });
    }

    return nodes.map((node) => ({
      ...node,
      position: positionMap.get(node.id) ?? node.position,
    }));
  } catch (err) {
    console.error('ELK layout failed, falling back to grid:', err);
    return fallbackGrid(nodes);
  }
}

/**
 * Lay out nodes using ELK.js for non-flow views (domain groups, bounded contexts).
 * Groups nodes by type and arranges them in a clean grid with proper spacing.
 */
export async function layoutByType(nodes: Node[]): Promise<Node[]> {
  if (nodes.length === 0) return [];

  // For non-flow views, use a box layout that respects node sizes
  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'rectpacking',
      'elk.spacing.nodeNode': '40',
      'elk.rectpacking.desiredRowCount': String(Math.ceil(Math.sqrt(nodes.length))),
    },
    children: nodes.map((node) => ({
      id: node.id,
      width: DEFAULT_NODE_WIDTH,
      height: DEFAULT_NODE_HEIGHT,
    })),
    edges: [] as any[],
  };

  try {
    const layout = await elk.layout(elkGraph);

    const positionMap = new Map<string, { x: number; y: number }>();
    for (const child of layout.children ?? []) {
      positionMap.set(child.id, { x: child.x ?? 0, y: child.y ?? 0 });
    }

    return nodes.map((node) => ({
      ...node,
      position: positionMap.get(node.id) ?? node.position,
    }));
  } catch (err) {
    console.error('ELK rectpacking failed, falling back to grid:', err);
    return fallbackGrid(nodes);
  }
}

/** Simple fallback grid layout if ELK fails */
function fallbackGrid(nodes: Node[]): Node[] {
  const cols = Math.ceil(Math.sqrt(nodes.length));
  return nodes.map((node, i) => ({
    ...node,
    position: { x: (i % cols) * 300, y: Math.floor(i / cols) * 120 },
  }));
}
