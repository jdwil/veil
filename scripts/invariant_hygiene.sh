#!/usr/bin/env bash
# INV-007: fail on new engine domain-knowledge literals (residual debt allowlisted).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ALLOW="${ROOT}/scripts/invariant_allowlist.txt"

DENY=(Aggregate BoundedContext ApplicationService DomainEvent crud_for_aggregate '"dep"')

is_allowed() {
  local tok="$1" file="$2"
  [[ -f "$ALLOW" ]] || return 1
  while IFS='|' read -r atok apath _anote; do
    [[ -z "${atok:-}" || "$atok" =~ ^# ]] && continue
    if [[ "$tok" == "$atok" && "$file" == *"$apath"* ]]; then
      return 0
    fi
  done < "$ALLOW"
  return 1
}

hits=0
while IFS= read -r -d '' f; do
  rel="${f#"$ROOT"/}"
  for tok in "${DENY[@]}"; do
    if grep -nF -- "$tok" "$f" >/tmp/inv_hits.txt 2>/dev/null; then
      while IFS= read -r line; do
        if is_allowed "$tok" "$rel"; then
          continue
        fi
        echo "INV-007 deny: token=$tok file=$rel:$line"
        hits=$((hits + 1))
      done < /tmp/inv_hits.txt
    fi
  done
done < <(find crates/veil-ir/src crates/veil-codegen/src crates/veil-parser/src crates/veil-server/src -name '*.rs' -print0 2>/dev/null)

rm -f /tmp/inv_hits.txt

if [[ $hits -gt 0 ]]; then
  echo ""
  echo "INV-007 failed: $hits unallowlisted hit(s)."
  echo "Add to scripts/invariant_allowlist.txt with ticket id if residual debt."
  exit 1
fi
echo "INV-007 ok: no unallowlisted domain literals in engine crates."
