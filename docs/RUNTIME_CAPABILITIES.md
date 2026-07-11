# Runtime capability matrix (PVR-001)

Status of public Bus / platform ops vs live host. Update when wiring changes.

| Capability | Bus / API | Status | Notes |
|------------|-----------|--------|-------|
| List products | `ListRepos`, `GET /api/projects` | **live** | projects hub FS |
| Create product | `CreateRepo`, `POST /api/projects` | **live** | `veil init` scaffold |
| Write file | `WriteFile` | **live** | under project root |
| Read file | `ReadFile` | **live** | under project root |
| List files | `ListFiles` | **live** | walk project root |
| Create branch | `CreateBranch` | **live** | `git branch` / gix |
| List branches | `ListBranches` | **live** | git |
| Get diff | `GetDiff` | **live** | `git diff` |
| Commit log | `GetCommitLog` | **live** | `git log` |
| Compile | `Compile` | **live** | `veil check` on package |
| Deploy local | `Deploy` | **live** | records path under `~/.veil/artifacts` |
| Config get/set | `GET /api/config` | **live** | config.json |
| Health | `GET /health` | **live** | |
| Agent turn | `HandleAgentMessage` | **partial** | proxies to agent if configured |
| IDE dual-loop | `/api/p/{name}/…` | **live** | veil-server multi |
| Shell UI | `GET /` | **live** | generated SPA preferred; static fallback |
| Registry UI | | **partial** | list layers dir |
| Cloud deploy | | **missing** | adapter-gated P3 |

`VEIL_RUNTIME_STUB=1` forces echo stubs (tests only).
