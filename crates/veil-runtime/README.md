# veil-runtime

Product host binary: IDE kernel (`veil-server`) + platform HTTP (repos, PRs, deploy).

```bash
cargo build --release -p veil-runtime
scripts/dev-stack.sh restart
```

- API: `http://127.0.0.1:8080`
- UI: `ui/` Vite on `:5180`
