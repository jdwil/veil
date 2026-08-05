<script lang="ts">
  let {
    open = false,
    title = 'Confirm',
    message = '',
    confirmLabel = 'Delete',
    cancelLabel = 'Cancel',
    destructive = true,
    onConfirm,
    onCancel,
  }: {
    open: boolean;
    title?: string;
    message?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    destructive?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') { onCancel(); }
    if (e.key === 'Enter') { onConfirm(); }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div class="confirm-backdrop" onclick={onCancel} role="presentation"></div>
  <div class="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
    <h3 id="confirm-title" class="confirm-title">{title}</h3>
    {#if message}
      <p class="confirm-message">{message}</p>
    {/if}
    <div class="confirm-actions">
      <button type="button" class="confirm-btn cancel" onclick={onCancel}>{cancelLabel}</button>
      <button type="button" class="confirm-btn {destructive ? 'destructive' : 'primary'}" onclick={onConfirm}>{confirmLabel}</button>
    </div>
  </div>
{/if}

<style>
  .confirm-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    z-index: 9998;
  }
  .confirm-dialog {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: var(--surface, #1e1e2e);
    border: 1px solid var(--border, #444);
    border-radius: 12px;
    padding: 24px;
    min-width: 320px;
    max-width: 420px;
    z-index: 9999;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }
  .confirm-title {
    margin: 0 0 8px;
    font-size: 16px;
    font-weight: 600;
    color: var(--text, #e0e0e0);
  }
  .confirm-message {
    margin: 0 0 20px;
    font-size: 13px;
    color: var(--text-secondary, #aaa);
    line-height: 1.4;
  }
  .confirm-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .confirm-btn {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
    transition: background 0.15s;
  }
  .confirm-btn.cancel {
    background: var(--surface-hover, #2a2a3e);
    color: var(--text, #e0e0e0);
  }
  .confirm-btn.cancel:hover {
    background: var(--surface-active, #333);
  }
  .confirm-btn.destructive {
    background: #dc2626;
    color: white;
  }
  .confirm-btn.destructive:hover {
    background: #b91c1c;
  }
  .confirm-btn.primary {
    background: var(--accent, #6366f1);
    color: white;
  }
  .confirm-btn.primary:hover {
    background: var(--accent-hover, #4f46e5);
  }
</style>
