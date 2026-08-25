import adapter from '@sveltejs/adapter-static';
export default {
    kit: {
        adapter: adapter({
            pages: 'build',
            assets: 'build',
            fallback: 'index.html',  // SPA fallback — all routes serve index.html
            precompress: false
        })
    }
};
