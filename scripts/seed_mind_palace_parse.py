#!/usr/bin/env python3
"""Parse VEIL agent SSE stream on stdin; print human-readable progress."""
from __future__ import annotations

import json
import sys


def flush(event: str | None, data_lines: list[str]) -> None:
    if event is None and not data_lines:
        return
    raw = "\n".join(data_lines).strip()
    if not raw:
        return
    try:
        obj = json.loads(raw)
    except json.JSONDecodeError:
        print(f"[{event or 'message'}] {raw[:200]}", flush=True)
        return

    ev = event or "message"
    if ev == "status":
        msg = obj.get("message") if isinstance(obj, dict) else obj
        print(f"… {msg}", flush=True)
    elif ev == "tool":
        name = obj.get("name", "?") if isinstance(obj, dict) else "?"
        detail = obj.get("detail", "") if isinstance(obj, dict) else ""
        d = detail if isinstance(detail, str) else json.dumps(detail)[:120]
        print(f"  tool {name}: {d}", flush=True)
    elif ev == "chunk":
        t = (obj.get("text") or "") if isinstance(obj, dict) else ""
        # Skip single-char typewriter noise; print longer pieces
        if len(t) > 1:
            print(t, end="", flush=True)
    elif ev == "done":
        print("\n\n=== done ===", flush=True)
        if isinstance(obj, dict):
            print(f"  backend: {obj.get('backend')}", flush=True)
            print(f"  ok: {obj.get('ok')}", flush=True)
            if obj.get("error"):
                print(f"  error: {obj.get('error')}", flush=True)
            for m in obj.get("messages") or []:
                if m.get("role") == "assistant":
                    body = (m.get("content") or "")[:2000]
                    print("--- assistant (truncated) ---", flush=True)
                    print(body, flush=True)
                    if len(m.get("content") or "") > 2000:
                        print("…", flush=True)
            tools = obj.get("tool_calls") or []
            if tools:
                print("--- tools used ---", flush=True)
                for t in tools:
                    print(
                        f"  - {t.get('name')}: {str(t.get('detail'))[:100]}",
                        flush=True,
                    )
    elif ev == "error":
        msg = obj.get("message") if isinstance(obj, dict) else obj
        print(f"ERROR: {msg}", flush=True)
    else:
        print(f"[{ev}] {str(obj)[:160]}", flush=True)


def main() -> int:
    event: str | None = None
    data_lines: list[str] = []
    for line in sys.stdin:
        line = line.rstrip("\n")
        if line.startswith(":"):
            continue
        if line.startswith("event:"):
            event = line[6:].strip()
            continue
        if line.startswith("data:"):
            data_lines.append(line[5:].lstrip())
            continue
        if line == "":
            flush(event, data_lines)
            event = None
            data_lines = []
    flush(event, data_lines)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
