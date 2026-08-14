<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	onMount(() => {
		const id = $page.url.searchParams.get('id') || $page.url.searchParams.get('slug');
		void goto(id ? `/projects/${encodeURIComponent(id)}` : '/projects', { replaceState: true });
	});
</script>
