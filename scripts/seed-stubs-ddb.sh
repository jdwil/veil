#!/usr/bin/env bash
# Deprecated alias — platform stubs live in S3 with DDB META pointers.
# See scripts/seed-stubs-platform.sh
exec "$(dirname "$0")/seed-stubs-platform.sh" "$@"
