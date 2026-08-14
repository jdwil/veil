<script lang="ts">

  import Modal from './Modal.svelte';

  interface Props {
    open?: boolean;
    title?: string;
    message?: string;
    default_value?: string;
    placeholder?: string;
    confirm_label?: string;
    cancel_label?: string;
    on_confirm?: (arg0: string) => void | null;
    on_cancel?: () => void | null;
    agent?: Record<string, unknown>;
  }
  let { open = $bindable(false), title = "Input", message = "", default_value = "", placeholder = "", confirm_label = "OK", cancel_label = "Cancel", on_confirm, on_cancel, agent = {  } }: Props = $props();

  let value: string = $state("");

  let veil_agent = $derived({ version: 1, role: "prompt-dialog", product: agent, runtime: { open, title } });

    $effect(() => { // reset_value
      if (open === true) {
        value = default_value;
      };
    });

  function do_cancel() {
    open = false;
    if (on_cancel !== null) {
      on_cancel();
    };
  }
  function do_confirm() {
    if (on_confirm !== null) {
      on_confirm(value);
    };
    open = false;
  }
</script>

<div class="dk-prompt-anchor" data-veil-role="prompt-dialog" data-veil-agent={JSON.stringify(veil_agent)} hidden aria-hidden="true"></div>
<Modal bind:open={open} title={title} size="sm" on_close={do_cancel} agent={agent}>
  {#if message}
    <p class="dk-dialog__message">{message}</p>
  {/if}
  <input class="input dk-dialog__input" type="text" placeholder={placeholder} bind:value data-veil-action="prompt-input" />
  {#snippet footer()}
    <button type="button" class="btn-outline" onclick={do_cancel} data-veil-action="cancel">{cancel_label}</button>
    <button type="button" class="btn-primary" onclick={do_confirm} data-veil-action="confirm">{confirm_label}</button>
  {/snippet}
</Modal>


<style>
  /* TODO: Add component styles */
</style>
