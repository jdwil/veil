<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	onMount(() => {
		void goto('/dashboard', { replaceState: true });
	});
</script>
