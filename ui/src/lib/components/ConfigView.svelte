<script lang="ts">

  import PageHeader from './PageHeader.svelte';
  import FormSection from './FormSection.svelte';
  import FormField from './FormField.svelte';

  let aws_region: string = $state('');
  let s3_bucket: string = $state('');
  let ddb_table: string = $state('');
  let llm_model: string = $state('');
  let saving: boolean = $state(false);
  let error: string = $state('');

  async function save() {
    saving = true;
    error = "";
    await (async () => { const __r = await fetch("/api/config", { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ aws_region, s3_bucket, ddb_table, llm_model }) }); if (!__r.ok) throw new Error(await __r.text()); const __t = await __r.text(); return __t ? JSON.parse(__t) : null; })();
    saving = false;
  }
</script>

<div class="config">
  <PageHeader title="Config" description="Runtime environment settings for this host." />
  <form class="dk-form card" onsubmit={(e) => { e.preventDefault(); save(); }}>
    <FormSection title="AWS" columns={2}>
      <FormField label="Region" bind:value={aws_region} placeholder="us-west-2" />
      <FormField label="S3 bucket" bind:value={s3_bucket} placeholder="veil-artifacts-dev" />
      <FormField label="DynamoDB table" bind:value={ddb_table} placeholder="veil-runtime-dev" />
      <FormField label="LLM model" bind:value={llm_model} placeholder="…" />
    </FormSection>
    {#if error}
      <p class="dk-error">{error}</p>
    {/if}
    <div class="actions">
      <button type="submit" class="btn-primary" disabled={saving}>
        {saving ? "Saving…" : "Save"}
      </button>
    </div>
  </form>
</div>


<style>
.config { max-width: 42rem; animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both; }
.dk-form { padding: 1.25rem 1.35rem 1.5rem; display: flex; flex-direction: column; gap: 1.5rem; }
.actions { display: flex; justify-content: flex-end; }

</style>
