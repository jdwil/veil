    // Auto-generated API proxy — forwards /api requests to http://127.0.0.1:3000
    import type { Handle } from '@sveltejs/kit';

    const API_PREFIX = '/api';
    const BACKEND = 'http://127.0.0.1:3000';

    export const handle: Handle = async ({ event, resolve }) => {
      if (event.url.pathname.startsWith(API_PREFIX)) {
        const target = BACKEND + event.url.pathname + event.url.search;
        const headers = new Headers(event.request.headers);
        headers.delete('host');
        const resp = await fetch(target, {
          method: event.request.method,
          headers,
          body: event.request.method !== 'GET' && event.request.method !== 'HEAD'
            ? await event.request.arrayBuffer()
            : undefined,
          duplex: 'half' as any,
        });
        return new Response(resp.body, {
          status: resp.status,
          statusText: resp.statusText,
          headers: resp.headers,
        });
      }
      return resolve(event);
    };