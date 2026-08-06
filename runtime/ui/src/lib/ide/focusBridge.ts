/**
 * Publish IDE selection into SessionFocus so the agent understands
 * "this component" / "this file" without extra user explanation.
 */
import { get } from 'svelte/store';
import { irGraph, selectedNodeId, diagnostics } from './store';
import { patchFocus } from '$lib/agent';

let unsubs: Array<() => void> = [];

export function startIdeFocusBridge(project: string): () => void {
	stopIdeFocusBridge();

	const publishSelection = () => {
		const id = get(selectedNodeId);
		const graph = get(irGraph);
		const diags = get(diagnostics);
		if (!id || !graph) {
			patchFocus({
				project,
				route: `/projects/${project}/ide`,
				construct: null,
				constructKind: null,
				selection: null,
				diagnostics: {
					count: diags.length,
					sample: diags.slice(0, 5).map((d) => ({
						severity: d.severity,
						message: d.message,
						node_name: d.node_name,
						code: d.code,
						hint: d.hint
					}))
				}
			});
			return;
		}
		const node = graph.nodes.find((n) => String(n.id) === String(id));
		if (!node) {
			patchFocus({
				project,
				route: `/projects/${project}/ide`,
				construct: null,
				constructKind: null,
				selection: null
			});
			return;
		}
		patchFocus({
			project,
			route: `/projects/${project}/ide`,
			construct: node.name,
			constructKind: node.kind,
			selection: {
				kind: node.kind,
				id: String(node.id),
				label: node.name
			},
			diagnostics: {
				count: diags.length,
				sample: diags.slice(0, 5).map((d) => ({
					severity: d.severity,
					message: d.message,
					node_name: d.node_name,
					code: d.code,
					hint: d.hint
				}))
			}
		});
	};

	unsubs = [
		selectedNodeId.subscribe(() => publishSelection()),
		irGraph.subscribe(() => publishSelection()),
		diagnostics.subscribe(() => publishSelection())
	];
	publishSelection();

	return stopIdeFocusBridge;
}

export function stopIdeFocusBridge() {
	for (const u of unsubs) u();
	unsubs = [];
}
