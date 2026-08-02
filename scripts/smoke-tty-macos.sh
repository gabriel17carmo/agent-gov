#!/usr/bin/env bash

set -Eeuo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf 'TTY smoke test skipped outside macOS\n'
  exit 0
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
binary="${1:-$repo_root/target/debug/agent-gov}"
transcript="$(mktemp "${TMPDIR:-/tmp}/agent-gov-tty.XXXXXX")"
state="$(mktemp -d "${TMPDIR:-/tmp}/agent-gov-tty-state.XXXXXX")"
trap 'rm -f "$transcript"; rm -rf "$state"' EXIT

AGENT_GOV_TEST_HOME="$state" script -q "$transcript" \
  "$binary" run --owner tty-smoke -- /bin/sh -c \
  'test -t 0 && printf "agent-gov-tty-ok\n"'

grep -q 'agent-gov-tty-ok' "$transcript"
printf 'TTY smoke test passed\n'
