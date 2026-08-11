/**
 * Format diagnostics as agent prompts and deliver them to the runtime agent.
 *
 * Native product IDE (`/projects/[id]/ide`) shares the shell with AgentDock —
 * no iframe. Prefer the runtime session (`$lib/agent`) so "Agent: fix all"
 * opens the dock and streams on the same conversation as Cmd+K.
 *
 * Legacy iframe embed still uses postMessage → parent `initIdeBridge`.
 */
import type { Diagnostic } from './store';
import { currentProjectParam } from './store';
import {
	agentSend,
	openAgentPanel,
	agentInsertToken,
	ideDiagnosticsSummary,
} from '$lib/agent/runtimeAgentSession';

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
		'After every edit: veil_check. Fix any new errors/warnings you introduced on this same turn.',
		'Git-shaped workflow (you decide branch/commit — do not ask the operator for every step):',
		'session_status → multi-step? create_branch → veil_check baseline → fix one class → write → veil_check → session_commit.',
		// Avoid bare tool tokens that false-trigger host platform-UX short-circuit
		// (parse_platform_ux_intent substring match). Name tools only inside SOP prose.
		'When the full task is done: open a PR for human review (title + description with per-slice rationales), then submit it. Do not open an empty create-form mid-fix.',
		'NEVER merge unless the operator explicitly asks to land. Humans review via the PR Wizard.',
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
 * Native shell: runtime AgentDock session.
 * Embedded iframe: postMessage → parent bridge.
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

	// Native product IDE — same app as AgentDock (no iframe).
	openAgentPanel();
	if (autoSend) {
		void agentSend(text);
	} else {
		agentInsertToken(text);
	}
}

function samplePayload(sample?: Diagnostic[]) {
	return (sample ?? []).slice(0, 5).map((d) => ({
		severity: d.severity,
		message: d.message,
		node_name: d.node_name,
		code: d.code,
		hint: d.hint,
	}));
}

/** Notify host of open diagnostic count (for empty-state chips). */
export function publishDiagnosticsSummary(
	count: number,
	sample?: Diagnostic[]
) {
	const payload = {
		count,
		project: currentProjectParam(),
		sample: samplePayload(sample),
	};

	if (isEmbeddedInShell()) {
		window.parent.postMessage(
			{
				type: 'ide:diagnostics-summary',
				payload,
			},
			'*'
		);
		return;
	}

	// Native: update the store AgentDock already reads.
	ideDiagnosticsSummary.set({
		count: payload.count,
		project: payload.project,
		sample: payload.sample,
	});
}
