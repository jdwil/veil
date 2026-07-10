# VCS model decision (RT-016)

**Decision:** keep **source of truth on the filesystem** for local dev
(`veil serve` project root). Platform object store holds **artifacts and
optional remote copies**, not the daily edit loop.

| Mode | Source | Meta | When |
|------|--------|------|------|
| Local IDE | disk `.veil` | optional sqlite later | default |
| Local platform | disk or content-addressed fs objects | `FileMetaStore` / sqlite | RT-010/011 |
| Remote deploy | object store (S3 adapter) | meta DB | RT-014+ |

**gix:** optional later for git-native operations inside platform; **not**
required for dual-loop. Prefer plain filesystem + hashes over embedding a full
git implementation in the engine.

**Not chosen:** putting full source trees in sqlite by default.
