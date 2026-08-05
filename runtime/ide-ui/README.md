# runtime/ide-ui — VEIL dual-loop IDE UI

Formerly monorepo-root `veil-viewer/`. Owned by **veil-runtime** and served
same-origin by `ProductHost` at `/viewer` (see `runtime/docs/ADR_SINGLE_PRODUCT_HOST.md`).

```bash
cd runtime/ide-ui && npm i
VEIL_VIEWER_BASE=/viewer npm run build
# output → build/; pure-runtime-build copies to runtime/bootstrap/static/viewer
```

Dev (optional standalone Vite): `npm run dev` with `VEIL_API_PORT` pointing at
**ProductHost** (not a separate multi `veil serve`).

---

# sv

Everything you need to build a Svelte project, powered by [`sv`](https://github.com/sveltejs/cli).

## Creating a project

If you're seeing this, you've probably already done this step. Congrats!

```sh
# create a new project
npx sv create my-app
```

To recreate this project with the same configuration:

```sh
# recreate this project
npx sv@0.16.1 create --template minimal --types ts --install npm veil-viewer
```

## Developing

Once you've created a project and installed dependencies with `npm install` (or `pnpm install` or `yarn`), start a development server:

```sh
npm run dev

# or start the server and open the app in a new browser tab
npm run dev -- --open
```

## Building

To create a production version of your app:

```sh
npm run build
```

You can preview the production build with `npm run preview`.

> To deploy your app, you may need to install an [adapter](https://svelte.dev/docs/kit/adapters) for your target environment.
