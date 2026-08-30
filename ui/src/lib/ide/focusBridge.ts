/**
 * Publish IDE selection + diagnostics into ideViewport / SessionFocus so the
 * agent understands "this component" / "this file" without extra explanation.
 */
import { get } from 'svelte/store';
import {
	irGraph,
	selectedNodeId,
	diagnostics,
	activeFileName,
	activeFileKind,
} from './store';
import {
	ideViewport,
	patchIdeViewport,
	flushIdeViewportToFocus,
	clearIdeViewport,
} from './ideViewport';

let unsubs: Array<() => void> = [];

export function startIdeFocusBridge(project: string): () => void {
	stopIdeFocusBridge();

	const publish = () => {
		const id = get(selectedNodeId);
		const graph = get(irGraph);
		const diags = get(diagnostics);
		const file = get(activeFileName);
		const fileKind = get(activeFileKind);
		const prev = get(ideViewport);

		const sample = diags.slice(0, 5).map((d) => ({
			severity: d.severity,
			message: d.message,
			node_name: d.node_name,
			code: d.code,
			hint: d.hint,
		}));

		const node =
			id && graph
				? graph.nodes.find((n) => String(n.id) === String(id))
				: undefined;

		const primaryPane = node ? 'canvas' : prev.primaryPane || 'outline';

		patchIdeViewport({
			project,
			file: file || null,
			fileKind: fileKind || null,
			construct: node?.name ?? null,
			constructKind: node?.kind ?? null,
			constructSubkind: node?.metadata?.subkind ?? null,
			selectionId: node ? String(node.id) : null,
			diagnosticsCount: diags.length,
			diagnosticsSample: sample,
			primaryPane,
		});
		flushIdeViewportToFocus();
	};

	unsubs = [
		selectedNodeId.subscribe(() => publish()),
		irGraph.subscribe(() => publish()),
		diagnostics.subscribe(() => publish()),
		activeFileName.subscribe(() => publish()),
	];
	patchIdeViewport({ project, primaryPane: 'canvas' });
	publish();

	return stopIdeFocusBridge;
}

export function stopIdeFocusBridge() {
	for (const u of unsubs) u();
	unsubs = [];
	clearIdeViewport();
}
