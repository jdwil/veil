<script lang="ts">

  interface Props {
    name?: string;
    subtitle?: string;
    show_avatar?: boolean;
    size?: string;
  }
  let { name = "", subtitle = "", show_avatar = true, size = "md" }: Props = $props();

  let letter = $derived("?");
</script>

{#if true}
{@const letter = (name || '?').toString().charAt(0).toUpperCase()}
<div class="dk-tile__identity">
  {#if show_avatar}
    <div class="dk-avatar" class:dk-avatar--sm={size === 'sm'} aria-hidden="true">{letter}</div>
  {/if}
  <div class="dk-tile__identity-text">
    <p class="dk-tile__name">{name}</p>
    {#if subtitle}
      <p class="dk-tile__meta">{subtitle}</p>
    {/if}
  </div>
</div>
{/if}


<style>
  /* TODO: Add component styles */
</style>
