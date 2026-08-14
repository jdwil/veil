import adapterAuto from '@sveltejs/adapter-auto';
import adapterStatic from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// Pure-runtime embed: VEIL_VIEWER_BASE=/viewer npm run build
// Dev (`npm run dev`) keeps base empty and adapter-auto.
const viewerBase = process.env.VEIL_VIEWER_BASE || '';
const embedStatic = Boolean(viewerBase);

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			paths: {
				base: viewerBase
			},
			adapter: embedStatic
				? adapterStatic({
						pages: 'build',
						assets: 'build',
						fallback: 'index.html',
						strict: false
					})
				: adapterAuto()
		})
	],
	// Source-export Svelte libs (github:jdwil/aether-ui) ship .svelte.ts with
	// TypeScript — prebundling fails on `import type`. Let Vite transform them.
	optimizeDeps: {
		exclude: ['@aether-ui/core']
	},
	ssr: {
		noExternal: ['@aether-ui/core']
	}
});
