# VEIL — Visual Engineering Intermediate Language

This is the product brief. Language mechanics live in
[`docs/LANGUAGE.md`](docs/LANGUAGE.md). Codegen lives in
[`docs/CODEGEN_TEMPLATES.md`](docs/CODEGEN_TEMPLATES.md). Repo layout is in
[`README.md`](README.md). The examples here are not the only legal
architecture.

VEIL is a small, indentation-based IR that agents write and humans oversee.
It compiles to real languages (Rust today; TypeScript and others on the
road) and shows up as a structural viewer you can actually review.

Licensed **AGPL-3.0-only** (JD Williams, jd@unsung-operators.com). Hosted
ProductHost and commercial licenses: same address.

The engine has a fixed core grammar. **Layers** teach domain and platform
words at runtime. **Backends** lower to real projects. The **runtime** is
where a human watches the work happen and signs it off.

The long bet is **expressiveness parity**: anything you can say in a major
application language can be represented in VEIL and lowered with the same
meaning. Semantic parity — not a keyword twin of Rust or TypeScript.

## Who does what

| Role | Job |
|------|-----|
| Agents | Primary authors. Write `.veil` cheaply; drive the runtime through the same surfaces the human sees. |
| Humans | Primary reviewers. Watch, steer in English, sign off on topology and critical bodies. Do not reconstruct the session from git blame. |
| Engine | Parse, check, lower. Zero domain knowledge. |
| Layers | Vocabulary, visuals, prompts, codegen opinions. |
| Runtime | Where the human watches. Mutations show up, get reviewed, get signed off. |

```bash
veil serve .          # project-root IDE: graph, check, edit, agent
veil check path.veil  # machine loop
veil gen path.veil    # lower to a target project
```

Optional local platform (fs+sqlite / cloud adapters) is power tooling, not a
gate. App harnesses are VEIL-authored (`@main`); see
[`docs/HARNESS.md`](docs/HARNESS.md). ProductHost:
`scripts/dev-stack.sh` (`ui/` + `crates/veil-runtime`).

## Laws

1. **Zero domain knowledge in the engine.** Permanent. `ctx` / `agg` /
   `dispatch` live in layers.
2. **Agents author; humans review topology + critical bodies.** Not every
   expression, not generated LoC by default.
3. **Two loops, both product.** Machine: parse → check → codegen →
   (optional) target compile. Human: an outstanding change set, closed by
   recorded sign-off.
4. **Expressiveness parity is semantic.** Keyword-for-keyword clones explode
   the core; they are rejected.
5. **Intelligent lowering, not token mapping.** Borrow when it's safe; emit
   what a target-language expert would write. String-patching and
   `if keyword == "handler"` are debt.
6. **Token efficiency.** Terse is the standard. Verbose is compatibility.
7. **Terseness never outranks diagnostics.** Sugar that silently miscompiles
   is a bug. Strict check is the agent default.
8. **Escape hatches are debt.** Raw blocks, untyped `Json`, stub-only calls,
   FFI — visible, scheduled to die.
9. **Layers own vocabulary, visuals, prompts, and pattern codegen.**
10. **Blessed paths are not the core.** `ddd` / `di` are defaults for service
    apps, not the only legal shape.
11. **No silent miscompile.** Unsupported target features fail at check.
12. **Authorship is visible.** Announce intent, then act on the real UI at a
    speed a human can follow. No parallel invisible backend.
13. **Outstanding changes are a product surface.** Git answers history.
    Review state answers "has a human signed this off?"
14. **Do not reinvent git.** Commits, branches, merge, log, and diff stay
    git's job.

## Runtime

The inner agent is the only primary author inside ProductHost. The agent
pane is the conversation. Everything else is what the human is watching.

Graphics do not speed agents. Fast diagnostics and deterministic codegen
are half the product. Review plus recorded sign-off is the other half.

Humans should usually look at topology (packages, modules, groups,
constructs, ports, wiring, expose contracts, annotations) and at critical
bodies (guards, orchestration, adapters, other high-risk expressions).
They should not need every expression, or the generated target, for routine
approval. Generated code stays one click away when you distrust a lowering
or are chasing a performance bug. Success is rarely needing it, not never.

Mutations go through the same contracts the UI uses (`data-veil-agent`,
structured edit APIs, `/api/repos`, Focus + Intent + Present). There is no
second agent backend that changes state in the dark.

When the agent fills a form or clicks a control, the UI should move the way
a careful human would: typing, focus, button press, in-flight disable,
progress on multi-step work. Default cadence is something a watcher can
follow. Fast-forward exists for power users and tests.

The moment the agent edits a project's files, that IDE comes forward.
Unreviewed diffs stay visually distinct. A session often touches several
small libraries and layers — the Projects page should show, at a glance,
what was touched, what is outstanding, and what needs sign-off.

After a coherent unit of work the agent presents the set ("created X, edited
Y, generated Z, here's why") and asks for sign-off. Approval is a recorded
action — button, command, or agent-mediated confirmation — suitable for
SOC 2. Reject and partial approve leave the rest outstanding. At the end of
a session the agent can roll up everything it touched across projects.

The agent must be able to list projects, open IDEs, bounce context, run
check/gen, open PRs, and do real work from layers and stubs — not just
puppet the chrome.

Edits go through `POST /api/edit`: update AST → re-serialize → validate →
write. Round-trips have to be deterministic. Pretty-print churn breaks
trust.

## Language

Three authoring layers:

1. **Core** — 7 construct shapes (`mod`, `struct`, `enum`, `trait`, `impl`,
   `fn`, `group`), 2 statement shapes (`call`, `if`), and the usual
   expression forms.
2. **Layers** (`.layer`) — domain and platform words, visuals, prompts,
   codegen.
3. **Application** (`.veil`) — written with the vocabulary those layers
   taught.

`.stub` files declare external crate/SDK APIs (`veil stub-gen <crate>`).
The viewer is the structural editor (palette, graph, property panels).
Source text stays the agent-native form.

A layer maps new words onto core shapes:

```
pkg ddd v1

  construct Context
    kw ctx
    mt mod
    visual
      icon "📦"
      color "#8b5cf6"
      label "Bounded Context"
```

Application code `use`s the layer:

```
pkg MyApp
  use ddd
  ctx Identity
    group domain
      agg Customer
        root
          id: UUID
          email: Email
      port CustomerRepo
        save(customer: Customer) -> Res!
        find(id: UUID) -> Res!<Opt<Customer>>
```

`mt` may name another construct (`lead → agg → struct`). Constraints and
statements follow the same is-a chain. A statement whose `mt` is
`Port.method` desugars at parse to a call; the source and viewer keep the
sugar.

The parser does not know what `ctx` means. The builder does not hardcode
`"Aggregate"`. Codegen does not special-case DDD. The viewer does not
hardcode icons. If someone ships `swiftui.layer`, the system works without
engine or viewer patches.

Engine heuristics that encode policy by magic names (`dep`, field-name
constructors, Bus-shaped routing) are invariant debt. Put the rule in a
layer; the engine only executes it.

Presentation grammar: [`docs/PRESENTATION.md`](docs/PRESENTATION.md).
Architecture / deploy: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Codegen

VEIL is not a keyword-to-syntax stamp. Lowering has to understand the
program and emit what a skilled human would write for that meaning.

| | Dumb mapper | What we want |
|--|-------------|--------------|
| Ownership | Clone everything | Borrow when safe, move on last use |
| Errors | `.unwrap()` or `?` on everything | The natural channel for that call site |
| Types | `impl Trait` / `_` | Concrete when known, generic when that's the point |
| Sharing | `Arc<Mutex<_>>` on every field | Interior mutability only when something mutates |
| Layers | Same struct regardless of construct | Value object ≠ entity ≠ event |
| Emit | Render strings, then patch them | Typed IR, then one emit pass |

Decisions happen on the typed IR. If inference cannot type a closure or a
match, it will clone defensively — that's how you get ugly output.

The engine may know the target language. It must not know `@dep`, `ctx`, or
`dispatch`. Those come from layers (`emit_to`, `lowers_to`, templates).
Adding a target should not mean a second 4k-line `expr_to_ts` that shares
nothing with Rust.

Each backend should publish a capability matrix (errors, async, ownership,
concurrency, modules). Unsupported constructs fail at check, not as wrong
code.

Quality bar for generated code:

- `cargo clippy` / `tsc --strict` clean, no suppressions
- Already in the shape `rustfmt` / `prettier` would produce
- No `todo!()` / `unreachable!()` / `unimplemented!()` on reachable paths
  unless the VEIL source marked them
- Reads like the target, not like a transliteration

```
veil gen app.veil -o ./out            # Rust
veil gen app.veil -o ./out -t ts      # TypeScript
```

Layer prompts for agents: [`docs/LAYER_PROMPTS.md`](docs/LAYER_PROMPTS.md).

## Now and next

The zero-domain-knowledge rule holds across parser, IR, viewer, and codegen.
Framework and domain code (HTTP harness, bus, auth, Svelte, `DomainError`,
tokio) lives in layers. Missing policy yields minimal valid output, not a
hidden default stack. Top-level unit is `pkg` only — never author `sol`.

Still compiled into the engine: the Rust and TypeScript backends. `emit_to`
and construct `lowers_to` are consumed. Role bindings for brand-new
construct shapes still need engine recognition. Template conditions are a
short list (`has_role`, `has_annotation`, `subkind ==`). Rust and TypeScript
share no lowering IR yet, so a third target would be a third implementation.
Language primitives (List `.get` / `.len`, Json extractors, Option unwrap)
belong in the backend because they are VEIL, not product vocabulary.
Product/SDK names stay in `.stub` / `.layer`.

Composability proof: `examples/crm.layer` on `ddd.layer`, plus
`examples/sales_crm.veil`, generate compiling Rust with no engine changes.

Next, in product order — not a sprint plan:

1. **Layer pass extensions.** Layers declare policy rules (async, derives,
   errors, null). The engine keeps ownership, inference, and expression
   lowering as compiled Rust. This extends `emit_to` / `lowers_to`; it does
   not replace the backends.
2. **Make the loops we already have actually good.** Check + deterministic
   codegen; topology / critical-body review; visible agency and sign-off;
   capability matrices that fail closed.
3. **More targets** (Go, Python, Swift, Kotlin) once (1) exists. Simple
   targets may be layer-only.
4. **Burn escape hatches** in real trees (`examples/`, runtime).

## How we know it's working

- Agent tokens (or steps) per feature vs writing the target language
- Human time to approve a structural change without opening generated LoC
- Time to understand "what just happened" after an agent turn — seconds,
  not a git archaeology session
- Share of agent changes signed off without opening generated files or blame
- Outstanding-change indicators on Projects after a multi-project session
- Fix-cycle time under `veil check` / compile
- Escape-hatch surface area (should fall)
- Capability violations caught at check, not in production
