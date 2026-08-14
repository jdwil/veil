<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	onMount(() => {
		void goto('/projects/new', { replaceState: true });
	});
</script>
