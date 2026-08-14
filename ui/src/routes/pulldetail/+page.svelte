<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	onMount(() => {
		const id = $page.url.searchParams.get('id');
		void goto(id ? `/pulls/${encodeURIComponent(id)}` : '/pulls', { replaceState: true });
	});
</script>
