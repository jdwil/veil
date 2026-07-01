import dagre from '@dagrejs/dagre';
import type { Node, Edge } from '@xyflow/svelte';

export function layoutNodes(
  nodes: Node[],
  edges: Edge[],
  direction: 'TB' | 'LR' = 'TB'
): Node[] {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: direction, nodesep: 80, ranksep: 100 });

  for (const node of nodes) {
    g.setNode(node.id, { width: 300, height: 100 });
  }

  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  return nodes.map(node => {
    const pos = g.node(node.id);
    return {
      ...node,
      position: {
        x: pos.x - 110,
        y: pos.y - 40,
      },
    };
  });
}
