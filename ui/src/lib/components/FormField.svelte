<script lang="ts">

  interface Props {
    id: string;
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
    onchange?: Fn | null;
    oninput?: Fn | null;
  }
  let { id, label, input_type = "text", required = false, placeholder = "", hint = "", error = "", value = $bindable(""), options = [], rows = 3, children, agent = {  }, onchange = undefined, oninput = undefined }: Props = $props();

  let veil_agent = $derived({ version: 1, role: "form-field", product: agent, runtime: { id, label, input_type, required } });
</script>

{#if true}
{@const empty = value === undefined || value === null || String(value).trim() === ''}
{@const incomplete = required && empty && !error}
{@const filled = required && !empty && !error}
<div
  class="dk-field"
  data-veil-role="form-field"
  data-veil-agent={JSON.stringify({ ...veil_agent, runtime: { ...veil_agent.runtime, empty, filled, error: error || undefined } })}
  data-veil-field={id}
>
  <label for={id} class="dk-field__label">
    {label}
    {#if required}
      <span
        class="dk-field__req"
        class:dk-field__req--filled={filled}
        title={empty ? 'Required' : 'Filled'}
      >●</span>
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
      <select {id} class="input" class:input-error={!!error} bind:value onchange={onchange}>
        {#each (options || []) as opt}
          <option value={opt.value}>{opt.label}</option>
        {/each}
      </select>
    {:else if input_type === 'textarea'}
      <textarea {id} class="input" class:input-error={!!error} {placeholder} rows={rows} bind:value oninput={oninput}></textarea>
    {:else}
      <input {id} type={input_type} class="input" class:input-error={!!error} {placeholder} bind:value oninput={oninput} />
    {/if}
  </div>
  {#if hint && !error}
    <p class="dk-field__hint">{hint}</p>
  {/if}
  {#if error}
    <p class="dk-field__error">{error}</p>
  {/if}
</div>
{/if}


<style>
  /* TODO: Add component styles */
</style>
