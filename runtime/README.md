# Runtime area — harness vs IDE vs platform (RT-006)

| Tool | Role |
|------|------|
| **`veil serve` in project root** | **Primary daily driver** — IDE, edit, check, agent |
| **App `@main` harness** | How *this* app runs (`docs/HARNESS.md`) |
| **Local platform (fs+sqlite)** | Optional power: object store, compile pipeline (RT-010+) |
| **Cloud adapters** | Provider-specific deploy (AWS/S3/DDB later) |

## Default story (RT-020)

```bash
# From your project (directory of .veil files / layers)
veil serve .
# open viewer → edit topology → check → agent prompt
```

No special platform daemon is required for the dual loop. Local platform
runtime is opt-in when you need multi-tenant object storage, deploy ports, etc.

## Authoring your own harness

See **`docs/HARNESS.md`**: `@main` / `@pvd` / `@dep` composition, `veil gen`,
gaps (RT-001b bin layout, Bus declare, …).

## Env (agent + models)

See **`docs/AGENT.md`** and **`docs/SERVER.md`**.

## Bootstrap

`runtime/bootstrap` is residual trampoline material — prefer VEIL-authored
`@main` (RT-000). Do not grow handwritten app registration as the only path.
