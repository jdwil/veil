<script lang="ts">
  import { goto } from '$app/navigation';
  import FormField from '$lib/components/FormField.svelte';
  import DetailShell from '$lib/components/DetailShell.svelte';

  let title = $state('');
  let description = $state('');
  let jira_ticket = $state('');
  let source_branch = $state('');
  let loading = $state(false);
  let error = $state('');

  async function handleSubmit() {
    if (!title.trim()) {
      error = 'Title is required';
      return;
    }
    loading = true;
    error = '';
    try {
      const resp = await fetch('/api/change_requests', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          title: title.trim(),
          description: description.trim(),
          jira_ticket: jira_ticket.trim(),
          source_branch:
            source_branch.trim() ||
            `feature/${title.trim().toLowerCase().replace(/\s+/g, '-')}`,
          author: 'jd',
        }),
      });
      if (!resp.ok) {
        const text = await resp.text();
        throw new Error(text || `HTTP ${resp.status}`);
      }
      const data = await resp.json();
      const id = data?.change_request?.id;
      if (id) {
        goto(`/changes/${id}`);
      } else {
        goto('/changes');
      }
    } catch (e: any) {
      error = e.message || 'Failed to create change request';
    } finally {
      loading = false;
    }
  }
</script>

<DetailShell title="Create Change Request" back_href="/changes">
  <!-- Present fill targets: formId=create-change, fields title/description/… -->
  <form
    id="create-change"
    data-veil-role="create-form"
    data-veil-form="create-change"
    on:submit|preventDefault={handleSubmit}
    class="form-body"
  >
    {#if error}
      <div class="error-msg">{error}</div>
    {/if}

    <FormField
      id="title"
      label="Title"
      bind:value={title}
      required={true}
      placeholder="e.g. Add authorization to relay handlers"
    />
    <FormField
      id="description"
      label="Description"
      bind:value={description}
      input_type="textarea"
      placeholder="What does this change do?"
      rows={6}
    />
    <FormField
      id="jira_ticket"
      label="Jira Ticket"
      bind:value={jira_ticket}
      placeholder="e.g. VEIL-123"
    />
    <FormField
      id="source_branch"
      label="Source Branch"
      bind:value={source_branch}
      placeholder="Auto-generated from title if empty"
    />

    <div class="actions">
      <a href="/changes" class="btn-ghost">Cancel</a>
      <button
        type="submit"
        class="btn-primary"
        data-veil-action="submit"
        disabled={loading || !title.trim()}
      >
        {#if loading}
          Creating…
        {:else}
          Create Change Request
        {/if}
      </button>
    </div>
  </form>
</DetailShell>

<style>
  .form-body {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    max-width: 600px;
  }
  .error-msg {
    padding: 0.5rem 0.75rem;
    background: rgba(239, 68, 68, 0.12);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: var(--dk-radius-sm, 0.55rem);
    color: #fecaca;
    font-size: 0.8rem;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-top: 0.5rem;
  }
</style>
