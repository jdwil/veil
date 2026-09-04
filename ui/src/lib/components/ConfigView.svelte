<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import FormSection from './FormSection.svelte';
	import FormField from './FormField.svelte';

	let projects_dir = $state('');
	let layers_dir = $state('');
	let show_core_layers = $state(false);
	let config_path = $state('');
	let veil_home = $state('');
	let version = $state('');
	let loading = $state(true);
	let saving = $state(false);
	let error = $state('');
	let saved = $state(false);
	let git_connected = $state(false);
	let git_login = $state('');
	let git_owner = $state('');
	let git_default = $state('s3');
	let git_hint = $state('');
	let git_error = $state('');
	let reference_dirs_text = $state('');
	let reference_roots: { id?: string; path?: string; usable?: boolean; skip_reason?: string | null }[] =
		$state([]);
	let search_paths_text = $state('');
	let search_roots: { id?: string; path?: string; usable?: boolean; skip_reason?: string | null }[] =
		$state([]);

	// Inner-agent provider config.
	type AgentProvider = {
		id: string;
		label: string;
		fields: string[];
		wired?: boolean;
		note?: string;
	};
	let agent_providers: AgentProvider[] = $state([]);
	let agent_provider = $state('');
	let agent_model = $state('');
	let agent_base_url = $state('');
	let agent_region = $state('');
	let agent_acp_command = $state('');
	let agent_acp_args = $state('');
	let agent_acp_agent = $state('');
	let agent_api_key_env = $state('');
	let agent_effective = $state('');
	let agent_env_override = $state(false);
	let agent_ready = $state(true);
	let agent_readiness_hint = $state('');

	function agentFields(): string[] {
		const p = agent_providers.find((x) => x.id === agent_provider);
		return p ? p.fields : [];
	}
	function agentHasField(f: string): boolean {
		return agentFields().includes(f);
	}

	function applyAgent(a: Record<string, unknown> | null | undefined) {
		if (!a || typeof a !== 'object') return;
		agent_provider = String((a.provider as string) ?? agent_provider ?? '');
		agent_model = String((a.model as string) ?? '');
		agent_base_url = String((a.base_url as string) ?? '');
		agent_region = String((a.region as string) ?? '');
		agent_acp_command = String((a.acp_command as string) ?? '');
		agent_acp_args = String((a.acp_args as string) ?? '');
		agent_acp_agent = String((a.acp_agent as string) ?? '');
		agent_api_key_env = String((a.api_key_env as string) ?? '');
		agent_effective = String((a.effective_provider as string) ?? '');
		agent_env_override = Boolean(a.env_override);
		agent_ready = a.ready === undefined ? true : Boolean(a.ready);
		agent_readiness_hint = String((a.readiness_hint as string) ?? '');
	}

	async function load() {
		loading = true;
		error = '';
		try {
			const r = await fetch('/api/config');
			if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
			const data = await r.json();
			projects_dir = String(data.projects_dir ?? '');
			layers_dir = String(data.layers_dir ?? '');
			show_core_layers = Boolean(data.show_core_layers);
			const refs = Array.isArray(data.reference_dirs)
				? data.reference_dirs.map((x: unknown) => String(x)).filter((s: string) => s.trim())
				: [];
			reference_dirs_text = refs.join('\n');
			const rr = data.reference_roots;
			if (rr && typeof rr === 'object' && Array.isArray((rr as { roots?: unknown }).roots)) {
				reference_roots = (rr as { roots: typeof reference_roots }).roots;
			} else {
				reference_roots = [];
			}
			const sp = Array.isArray(data.search_paths)
				? data.search_paths.map((x: unknown) => String(x)).filter((s: string) => s.trim())
				: [];
			search_paths_text = sp.join('\n');
			const sr = data.search_roots;
			if (sr && typeof sr === 'object' && Array.isArray((sr as { roots?: unknown }).roots)) {
				search_roots = (sr as { roots: typeof search_roots }).roots;
			} else {
				search_roots = [];
			}
			if (Array.isArray(data.agent_providers)) {
				agent_providers = data.agent_providers as AgentProvider[];
			}
			applyAgent(data.agent as Record<string, unknown>);
			if (!agent_provider && agent_providers.length) {
				agent_provider = agent_providers[0].id;
			}
			config_path = String(data.config_path ?? '');
			veil_home = String(data.veil_home ?? '');
			version = String(data.version ?? '');
			try {
				const g = await fetch('/api/git/status');
				if (g.ok) {
					const gs = await g.json();
					git_connected = Boolean(gs.connected);
					git_login = String(gs.login ?? '');
					git_owner = String(gs.owner ?? '');
					git_default = String(gs.default_origin ?? 's3');
					git_hint = String(gs.hint ?? '');
					git_error = String(gs.error ?? '');
				}
			} catch {
				/* optional */
			}
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load config';
		} finally {
			loading = false;
		}
	}

	async function save() {
		saving = true;
		error = '';
		saved = false;
		try {
			const r = await fetch('/api/config', {
				method: 'PATCH',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					projects_dir: projects_dir.trim(),
					layers_dir: layers_dir.trim(),
					show_core_layers,
					reference_dirs: reference_dirs_text
						.split('\n')
						.map((s) => s.trim())
						.filter((s) => s.length > 0 && !s.startsWith('#')),
					search_paths: search_paths_text
						.split('\n')
						.map((s) => s.trim())
						.filter((s) => s.length > 0 && !s.startsWith('#')),
					agent: {
						provider: agent_provider.trim(),
						model: agent_model.trim() || null,
						base_url: agent_base_url.trim() || null,
						region: agent_region.trim() || null,
						acp_command: agent_acp_command.trim() || null,
						acp_args: agent_acp_args.trim() || null,
						acp_agent: agent_acp_agent.trim() || null,
						api_key_env: agent_api_key_env.trim() || null
					}
				}),
			});
			if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
			const data = await r.json();
			projects_dir = String(data.projects_dir ?? projects_dir);
			layers_dir = String(data.layers_dir ?? layers_dir);
			show_core_layers = Boolean(data.show_core_layers);
			if (Array.isArray(data.reference_dirs)) {
				reference_dirs_text = data.reference_dirs.map((x: unknown) => String(x)).join('\n');
			}
			const rr = data.reference_roots;
			if (rr && typeof rr === 'object' && Array.isArray((rr as { roots?: unknown }).roots)) {
				reference_roots = (rr as { roots: typeof reference_roots }).roots;
			}
			if (Array.isArray(data.search_paths)) {
				search_paths_text = data.search_paths.map((x: unknown) => String(x)).join('\n');
			}
			const sr = data.search_roots;
			if (sr && typeof sr === 'object' && Array.isArray((sr as { roots?: unknown }).roots)) {
				search_roots = (sr as { roots: typeof search_roots }).roots;
			}
			if (Array.isArray(data.agent_providers)) {
				agent_providers = data.agent_providers as AgentProvider[];
			}
			applyAgent(data.agent as Record<string, unknown>);
			config_path = String(data.config_path ?? config_path);
			saved = true;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to save config';
		} finally {
			saving = false;
		}
	}

	onMount(() => {
		void load();
	});
</script>

<div class="config">
	<PageHeader title="Config" description="Local host settings, layer catalog, and GitHub origin." />
	{#if loading}
		<p class="hint">Loading…</p>
	{:else}
		<form
			class="dk-form card"
			onsubmit={(e) => {
				e.preventDefault();
				void save();
			}}
		>
			<FormSection title="Paths" columns={1}>
				<FormField
					id="projects_dir"
					label="Projects directory"
					bind:value={projects_dir}
					required={true}
					placeholder="~/veil-projects"
				/>
				<FormField
					id="layers_dir"
					label="Layers directory"
					bind:value={layers_dir}
					placeholder="optional override"
				/>
			</FormSection>
			<FormSection title="Reference trees" columns={1}>
				<FormField
					id="reference_dirs"
					label="Local code the agent may read"
					input_type="textarea"
					rows={4}
					bind:value={reference_dirs_text}
					placeholder={'~/src/legacy-app\nshop=~/code/shop'}
					hint="Read-only. One path per line (optional name=/path). The inner agent uses reference_read here when converting existing code to VEIL. It cannot write these trees. Env VEIL_REFERENCE_DIRS overlays this list."
				/>
				{#if reference_roots.length}
					<ul class="git-facts">
						{#each reference_roots as r}
							<li>
								<code>{r.id || 'root'}</code>
								{r.path || ''}
								{#if r.usable === false}
									— skipped{r.skip_reason ? `: ${r.skip_reason}` : ''}
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</FormSection>
			<FormSection title="Search paths (resolution points)" columns={1}>
				<FormField
					id="search_paths"
					label="Repos/dirs the resolver treats as roots"
					input_type="textarea"
					rows={4}
					bind:value={search_paths_text}
					placeholder={'~/dev/veil-libs\nlibs=~/dev/veil-libs'}
					hint="Resolved-from (unlike Reference trees). A `use <name>` in a consumer project resolves layers/stubs/library .veil against these roots — after local/project and [dependencies], before any remote registry. One path per line (optional name=/abs/path). Env VEIL_SEARCH_PATHS overlays this list."
				/>
				{#if search_roots.length}
					<ul class="git-facts">
						{#each search_roots as r}
							<li>
								<code>{r.id || 'root'}</code>
								{r.path || ''}
								{#if r.usable === false}
									— skipped{r.skip_reason ? `: ${r.skip_reason}` : ''}
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</FormSection>
			<FormSection title="Inner agent" columns={1}>
				<FormField
					id="agent_provider"
					label="Provider"
					input_type="select"
					bind:value={agent_provider}
					options={agent_providers.map((p) => ({ value: p.id, label: p.label }))}
					hint="The model backend the runtime agent uses. Env vars (VEIL_MODEL_PROVIDER, VEIL_ACP_*) always override this. Saved to ~/.veil/config.json."
				/>
				{#if agentHasField('acp_command')}
					<FormField
						id="agent_acp_command"
						label="ACP command"
						bind:value={agent_acp_command}
						placeholder="kiro-cli"
						hint="Command to spawn the external agent (default kiro-cli)."
					/>
				{/if}
				{#if agentHasField('acp_args')}
					<FormField
						id="agent_acp_args"
						label="ACP args"
						bind:value={agent_acp_args}
						placeholder="acp"
					/>
				{/if}
				{#if agentHasField('acp_agent')}
					<FormField
						id="agent_acp_agent"
						label="ACP agent name"
						bind:value={agent_acp_agent}
						placeholder="veil"
						hint="Kiro --agent name (optional)."
					/>
				{/if}
				{#if agentHasField('region')}
					<FormField
						id="agent_region"
						label="AWS region"
						bind:value={agent_region}
						placeholder="us-east-1"
						hint="Bedrock region. Note: Bedrock completions are config-only in v1 (selection + readiness work; not yet wired to a live client)."
					/>
				{/if}
				{#if agentHasField('base_url')}
					<FormField
						id="agent_base_url"
						label="Base URL"
						bind:value={agent_base_url}
						placeholder="https://openrouter.ai/api/v1"
						hint="OpenAI-compatible endpoint (BYOK gateway or Ollama)."
					/>
				{/if}
				{#if agentHasField('api_key_env')}
					<FormField
						id="agent_api_key_env"
						label="API key env var name"
						bind:value={agent_api_key_env}
						placeholder="OPENAI_API_KEY"
						hint="The NAME of an env var that holds your API key — not the key itself. Keys are never stored in config.json."
					/>
				{/if}
				{#if agentHasField('model')}
					<FormField
						id="agent_model"
						label="Model"
						bind:value={agent_model}
						placeholder="(provider default)"
						hint="Model id/name (optional; provider default when blank)."
					/>
				{/if}
				<ul class="git-facts">
					<li>
						Effective provider: <code>{agent_effective || agent_provider || '—'}</code>
						{#if agent_env_override}
							<span class="warn"> (forced by env var)</span>
						{/if}
					</li>
					<li>
						Readiness: {agent_ready ? '✓ ready' : '⚠ not ready'}
						{#if agent_readiness_hint}
							— {agent_readiness_hint}
						{/if}
					</li>
				</ul>
				<p class="hint">
					Saved to config, but the runtime reads the provider at startup.
					<strong>Restart the runtime to apply.</strong>
				</p>
			</FormSection>
			<label class="check">
				<input type="checkbox" bind:checked={show_core_layers} />
				Show core layers in the IDE
			</label>
			{#if error}
				<p class="dk-error">{error}</p>
			{/if}
			{#if saved}
				<p class="ok">Saved.</p>
			{/if}
			<div class="actions">
				<button type="submit" class="btn-primary" disabled={saving || !projects_dir.trim()}>
					{saving ? 'Saving…' : 'Save'}
				</button>
			</div>
			{#if config_path || veil_home}
				<p class="hint">
					{#if version}version {version} · {/if}
					{#if config_path}{config_path}{/if}
					{#if veil_home}<br />home {veil_home}{/if}
				</p>
			{/if}
		</form>
		<section class="card git-card">
			<h2 class="git-title">GitHub</h2>
			<p class="hint">
				New projects store source on GitHub when a token and
				<code>VEIL_GITHUB_OWNER</code> are set. Restart ProductHost after changing env.
			</p>
			<ul class="git-facts">
				<li>Account: {git_connected ? git_login || 'connected' : 'not connected'}</li>
				<li>Owner for new repos: {git_owner || '—'}</li>
				<li>Default origin: {git_default}</li>
			</ul>
			{#if git_error}
				<p class="dk-error">{git_error}</p>
			{/if}
			{#if git_hint}
				<p class="hint">{git_hint}</p>
			{/if}
		</section>
	{/if}
</div>

<style>
	.config {
		max-width: 42rem;
		animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both;
	}
	.dk-form {
		padding: 1.25rem 1.35rem 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
	}
	.check {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.9rem;
	}
	.hint {
		margin: 0;
		font-size: 0.8rem;
		opacity: 0.7;
	}
	.git-card {
		margin-top: 1.25rem;
		padding: 1.25rem 1.35rem;
	}
	.git-title {
		margin: 0 0 0.5rem;
		font-size: 1rem;
	}
	.git-facts {
		margin: 0.75rem 0;
		padding-left: 1.1rem;
		font-size: 0.9rem;
	}
	.ok {
		margin: 0;
		font-size: 0.85rem;
		color: #86efac;
	}
	.warn {
		color: #fbbf24;
	}
</style>
