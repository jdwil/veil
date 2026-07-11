# VCS model decision (RT-016)

**Decision:** keep **source of truth on the filesystem** for local dev
(`veil serve` project root). Platform object store holds **artifacts and
optional remote copies**, not the daily edit loop.

| Mode | Source | Meta | When |
|------|--------|------|------|
| Local IDE | disk `.veil` in a **project git repo** | optional sqlite later | default |
| Runtime local | **projects directory** of independent git repos; multi-tab IDE | `FileMetaStore` / sqlite | RT-010/011 |
| Local platform | disk or content-addressed fs objects | `FileMetaStore` / sqlite | RT-010/011 |
| Remote deploy | object store (S3 adapter) | meta DB | RT-014+ |

**gix:** optional later for git-native operations inside platform; **not**
required for dual-loop. Prefer plain filesystem + hashes over embedding a full
git implementation in the engine. Creating a project from the runtime UX
**does** `git init` a new repo under the configured projects directory
(see [`PROJECT_LAYOUT.md`](PROJECT_LAYOUT.md)).

**Not chosen:** putting full source trees in sqlite by default.
