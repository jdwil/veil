<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		id?: string;
		label: string;
		input_type?: string;
		required?: boolean;
		placeholder?: string;
		hint?: string;
		error?: string;
		value?: string;
		options?: Record<string, unknown>[];
		rows?: number;
		children?: Snippet | null;
		agent?: Record<string, unknown>;
		onchange?: ((e: Event) => void) | null;
		oninput?: ((e: Event) => void) | null;
	}
	let {
		id,
		label,
		input_type = 'text',
		required = false,
		placeholder = '',
		hint = '',
		error = '',
		value = $bindable(''),
		options = [],
		rows = 3,
		children,
		agent = {},
		onchange = undefined,
		oninput = undefined,
	}: Props = $props();

	let field_id = $derived(
		id ||
			label
				.toLowerCase()
				.replace(/[^a-z0-9]+/g, '-')
				.replace(/^-|-$/g, '') ||
			'field'
	);
	let veil_agent = $derived({
		version: 1,
		role: 'form-field',
		product: agent,
		runtime: { id: field_id, label, input_type, required },
	});

	function fire_input(e: Event) {
		oninput?.(e);
		onchange?.(e);
	}
	function fire_change(e: Event) {
		onchange?.(e);
		oninput?.(e);
	}
</script>

{@const empty = value === undefined || value === null || String(value).trim() === ''}
{@const incomplete = required && empty && !error}
{@const filled = required && !empty && !error}
<div
	class="dk-field"
	data-veil-role="form-field"
	data-veil-agent={JSON.stringify({
		...veil_agent,
		runtime: { ...veil_agent.runtime, empty, filled, error: error || undefined },
	})}
	data-veil-field={field_id}
>
	<label for={field_id} class="dk-field__label">
		{label}
		{#if required}
			<span class="dk-field__req" class:dk-field__req--filled={filled} title={empty ? 'Required' : 'Filled'}
				>●</span
			>
		{/if}
	</label>
	<div class="dk-field__control">
		<div
			class="dk-field__bar"
			class:dk-field__bar--incomplete={incomplete}
			class:dk-field__bar--filled={filled}
			class:dk-field__bar--error={!!error}
		></div>
		{#if children}
			{@render children()}
		{:else if input_type === 'select'}
			<select
				id={field_id}
				class="input"
				class:input-error={!!error}
				bind:value
				onchange={fire_change}
				oninput={fire_input}
			>
				{#each options || [] as opt}
					<option value={opt.value}>{opt.label}</option>
				{/each}
			</select>
		{:else if input_type === 'textarea'}
			<textarea
				id={field_id}
				class="input"
				class:input-error={!!error}
				{placeholder}
				{rows}
				bind:value
				oninput={fire_input}
				onchange={fire_change}
			></textarea>
		{:else}
			<input
				id={field_id}
				type={input_type}
				class="input"
				class:input-error={!!error}
				{placeholder}
				bind:value
				oninput={fire_input}
				onchange={fire_change}
			/>
		{/if}
	</div>
	{#if hint && !error}
		<p class="dk-field__hint">{hint}</p>
	{/if}
	{#if error}
		<p class="dk-field__error">{error}</p>
	{/if}
</div>
