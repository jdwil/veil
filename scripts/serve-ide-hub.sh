#!/usr/bin/env bash
# Deprecated. Dual `veil serve --multi` is gone.
# Product UX is one ProductHost: scripts/dev-stack.sh
set -euo pipefail
echo "serve-ide-hub.sh is retired (single ProductHost)."
echo "Use:  scripts/dev-stack.sh restart"
echo "See:  docs/ADR_SINGLE_PRODUCT_HOST.md"
exit 1
