# Success metrics instrumentation (PAR-010)

## Machine-readable CLI

```bash
veil check path/to/pkg.veil --json
```

JSON fields:

| Field | Meaning |
|-------|---------|
| `ok` | no errors |
| `error_count` / `warning_count` | diagnostic totals |
| `node_count` / `edge_count` | IR size |
| `duration_ms` | check wall time |
| `escape_hatch.*` | raw / empty_adapter / external / json_boundary / total |
| `diagnostics[]` | severity, code, message, node_name |
| `target` / `layers` / `package` | context |

Exit code `1` when `error_count > 0` (CI-friendly).

Human path still prints duration and escape-hatch summary lines.

## CI dashboard hooks

Pipe JSON to your aggregator:

```bash
veil check examples/pure_lib.veil --json > report.json
jq '{ok, duration_ms, escape: .escape_hatch.total}' report.json
```

Track over time: diagnostic counts, escape-hatch debt, check duration, dual-loop
failures on PR.

## Human time-to-approve (manual checklist)

Until IDE telemetry exists, measure dual-loop quality by hand:

1. Pick a representative change (rename, field add, service stub).
2. Time: open package → check green → agent or human edit → re-check → approve.
3. Record: minutes, escape-hatch count before/after, whether generated code was
   inspected (should trend **down** for structural edits).
4. Note blockers: missing construct, capability fail, template debt.

Mission success = shorter approve loops + fewer raw-surface escapes, not more
generated LoC read.
