#!/usr/bin/env bash

set -Eeuo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/agent-gov-installer-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fake_bin="$test_root/fake-bin"
payload_dir="$test_root/payload"
install_dir="$test_root/install"
mkdir -p "$fake_bin" "$payload_dir" "$install_dir"

printf '%s\n' '#!/usr/bin/env bash' \
  'case "${1:-}" in' \
  '  --version) printf "%s\n" "agent-gov 0.1.0" ;;' \
  '  install) shift; printf "%s\n" "$*" > "$INSTALLER_TEST_LOG" ;;' \
  '  doctor) printf "%s\n" "[warn] rtk: integration disabled"; exit 1 ;;' \
  '  *) exit 0 ;;' \
  'esac' > "$payload_dir/agent-gov"
chmod 0755 "$payload_dir/agent-gov"
shasum -a 256 "$payload_dir/agent-gov" > "$payload_dir/agent-gov.sha256"

printf '%s\n' '#!/usr/bin/env bash' \
  'case "${1:-}" in' \
  '  -s) printf "%s\n" "Darwin" ;;' \
  '  -m) printf "%s\n" "arm64" ;;' \
  '  *) exit 1 ;;' \
  'esac' > "$fake_bin/uname"
chmod 0755 "$fake_bin/uname"

printf '%s\n' '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'destination=""' \
  'url=""' \
  'while [[ "$#" -gt 0 ]]; do' \
  '  case "$1" in' \
  '    --output) destination="$2"; shift 2 ;;' \
  '    http*) url="$1"; shift ;;' \
  '    *) shift ;;' \
  '  esac' \
  'done' \
  'case "$url" in' \
  '  */agent-gov.sha256) cp "$INSTALLER_TEST_PAYLOAD/agent-gov.sha256" "$destination" ;;' \
  '  */agent-gov) cp "$INSTALLER_TEST_PAYLOAD/agent-gov" "$destination" ;;' \
  '  *) exit 22 ;;' \
  'esac' > "$fake_bin/curl"
chmod 0755 "$fake_bin/curl"

export INSTALLER_TEST_LOG="$test_root/install.log"
export INSTALLER_TEST_PAYLOAD="$payload_dir"
PATH="$fake_bin:$PATH" bash "$repo_root/install-agent-gov.sh" \
  --bin-dir "$install_dir" --agents cursor > "$test_root/output.log"

test -x "$install_dir/agent-gov"
test "$(cat "$INSTALLER_TEST_LOG")" = "--agents cursor"
grep -q "checksum" "$repo_root/install-agent-gov.sh"
grep -q "doctor completed with preview warnings" "$test_root/output.log"

printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$fake_bin/rtk"
chmod 0755 "$fake_bin/rtk"
PATH="$fake_bin:$PATH" bash "$repo_root/install-agent-gov.sh" \
  --bin-dir "$install_dir" --with-rtk > "$test_root/rtk-output.log"
test "$(cat "$INSTALLER_TEST_LOG")" = "--agents claude,cursor --with-rtk"

cp "$install_dir/agent-gov" "$test_root/installed-before-corruption"
printf '%064d  agent-gov\n' 0 > "$payload_dir/agent-gov.sha256"
if PATH="$fake_bin:$PATH" bash "$repo_root/install-agent-gov.sh" \
  --bin-dir "$install_dir" --no-hooks > "$test_root/corrupt.log" 2>&1; then
  printf '%s\n' "installer accepted a corrupt checksum" >&2
  exit 1
fi
cmp "$test_root/installed-before-corruption" "$install_dir/agent-gov"
grep -q "checksum verification failed" "$test_root/corrupt.log"

printf '%s\n' "installer tests passed"
