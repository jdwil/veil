<script lang="ts">

  import Modal from './Modal.svelte';

  interface Props {
    open?: boolean;
    title?: string;
    message?: string;
    confirm_label?: string;
    cancel_label?: string;
    variant?: string;
    busy?: boolean;
    on_confirm?: () => void | null;
    on_cancel?: () => void | null;
    agent?: Record<string, unknown>;
  }
  let { open = $bindable(false), title = "Confirm", message = "Are you sure?", confirm_label = "Confirm", cancel_label = "Cancel", variant = "default", busy = false, on_confirm, on_cancel, agent = {  } }: Props = $props();

  let veil_agent = $derived({ version: 1, role: "confirm-dialog", product: agent, runtime: { open, title, variant, busy } });

  function do_cancel() {
    if (busy === false) {
      open = false;
      if (on_cancel !== null) {
        on_cancel();
      };
    };
  }
  function do_confirm() {
    if (busy === false) {
      if (on_confirm !== null) {
        on_confirm();
      };
      open = false;
    };
  }
</script>

<div class="dk-confirm-anchor" data-veil-role="confirm-dialog" data-veil-agent={JSON.stringify(veil_agent)} hidden aria-hidden="true"></div>
<Modal bind:open={open} title={title} size="sm" on_close={do_cancel} agent={agent}>
  <p class="dk-dialog__message">{message}</p>
  {#snippet footer()}
    <button type="button" class="btn-outline" disabled={busy} onclick={do_cancel} data-veil-action="cancel">{cancel_label}</button>
    <button
      type="button"
      class={variant === 'danger' ? 'btn-danger' : 'btn-primary'}
      disabled={busy}
      onclick={do_confirm}
      data-veil-action="confirm"
    >{busy ? '…' : confirm_label}</button>
  {/snippet}
</Modal>


<style>
  /* TODO: Add component styles */
</style>
