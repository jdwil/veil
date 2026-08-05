/**
 * Format diagnostics as agent prompts and deliver them to the runtime agent
 * (parent shell when embedded) and/or local IDE agent session.
 */
import type { Diagnostic } from './store';
import { agentSend } from './agentSession';
import { currentProjectParam } from './store';

function formatOne(d: Diagnostic, index?: number): string {
	const n = index != null ? `${index + 1}. ` : '';
	const sev = d.severity ?? 'Issue';
	const code = d.code ? ` [${d.code}]` : '';
	const where = d.node_name ? ` @ ${d.node_name}` : '';
	const hint = d.hint ? `\n   Hint: ${d.hint}` : '';
	return `${n}${sev}${code}${where}: ${d.message}${hint}`;
}

export function formatIssuePrompt(
	diags: Diagnostic[],
	opts: { construct?: string; project?: string | null; all?: boolean }
): string {
	const project = opts.project ?? currentProjectParam() ?? 'this project';
	const scope = opts.construct
		? `construct \`${opts.construct}\` in project \`${project}\``
		: `project \`${project}\``;

	const list = diags.map((d, i) => formatOne(d, i)).join('\n');
	const heading = opts.all
		? `Investigate and fix all open issues for ${scope}.`
		: diags.length === 1
			? `Investigate and fix this issue on ${scope}.`
			: `Investigate and fix these issues on ${scope}.`;

	return [
		heading,
		'',
		'Use IDE / project tools as needed (read source, apply edits, re-check). Prefer minimal correct fixes.',
		'',
		'## Issues',
		list || '(no issue details)',
	].join('\n');
}

/** True when running as iframe embed inside the runtime shell. */
export function isEmbeddedInShell(): boolean {
	if (typeof window === 'undefined') return false;
	try {
		return window.parent != null && window.parent !== window;
	} catch {
		return true;
	}
}

/**
 * Open the agent UI and submit a prompt (or seed composer).
 * Embedded: postMessage → runtime AgentDock.
 * Standalone: local agentSession.
 */
export function askAgent(prompt: string, opts?: { autoSend?: boolean }) {
	const text = prompt.trim();
	if (!text) return;
	const autoSend = opts?.autoSend !== false;

	if (isEmbeddedInShell()) {
		window.parent.postMessage(
			{
				type: 'ide:agent-prompt',
				payload: { text, autoSend },
			},
			'*'
		);
		return;
	}

	// Standalone / agent rail in IDE
	void agentSend(text);
}

/** Notify host of open diagnostic count (for empty-state chips). */
export function publishDiagnosticsSummary(
	count: number,
	sample?: Diagnostic[]
) {
	if (!isEmbeddedInShell()) return;
	window.parent.postMessage(
		{
			type: 'ide:diagnostics-summary',
			payload: {
				count,
				project: currentProjectParam(),
				sample: (sample ?? []).slice(0, 5).map((d) => ({
					severity: d.severity,
					message: d.message,
					node_name: d.node_name,
					code: d.code,
					hint: d.hint,
				})),
			},
		},
		'*'
	);
}
