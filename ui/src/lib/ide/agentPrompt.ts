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
		'Host owns the coding workflow (step runner — not prompt-only):',
		'1. run_coding_plan({ plan: "coding.fix_diagnostics", action: "start", request: <this text> }).',
		'2. Host matches open unmerged PRs (scope/branch/phrases) — auto-bind, Present modal if ambiguous, or new.',
		'3. Perform next.tool from the result (session_status / write_source / veil_check / session_commit).',
		'4. After each successful agent step: run_coding_plan({ plan: "coding.fix_diagnostics", action: "next" }).',
		'5. Trust host_check / HOST_CHECK_SEVERITY — never claim clean if errors. create_pr/submit_pr aliases OK.',
		'6. Finish step opens/submits the pull request for PR Wizard. NEVER merge unless operator asks.',
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
