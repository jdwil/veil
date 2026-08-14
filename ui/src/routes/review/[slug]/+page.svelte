<script lang="ts">
	import { page } from '$app/stores';
	import ReviewSignOffView from '$lib/components/ReviewSignOffView.svelte';
	const slug = $derived($page.params.slug ?? '');
</script>

<ReviewSignOffView {slug} />
