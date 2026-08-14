<script lang="ts">

  import Modal from './Modal.svelte';

  interface Props {
    open?: boolean;
    title?: string;
    message?: string;
    ok_label?: string;
    on_ok?: () => void | null;
    agent?: Record<string, unknown>;
  }
  let { open = $bindable(false), title = "Notice", message = "", ok_label = "OK", on_ok, agent = {  } }: Props = $props();

  let veil_agent = $derived({ version: 1, role: "alert-dialog", product: agent, runtime: { open, title } });

  function do_ok() {
    open = false;
    if (on_ok !== null) {
      on_ok();
    };
  }
</script>

<div class="dk-alert-anchor" data-veil-role="alert-dialog" data-veil-agent={JSON.stringify(veil_agent)} hidden aria-hidden="true"></div>
<Modal bind:open={open} title={title} size="sm" on_close={do_ok} agent={agent}>
  <p class="dk-dialog__message">{message}</p>
  {#snippet footer()}
    <button type="button" class="btn-primary" onclick={do_ok} data-veil-action="ok">{ok_label}</button>
  {/snippet}
</Modal>


<style>
  /* TODO: Add component styles */
</style>
