#!/usr/bin/env bash
# The single-quoted string below is source code for a generated test double.
# shellcheck disable=SC2016

set -Eeuo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/agent-gov-release-plan.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

remote="$test_root/remote.git"
work="$test_root/work"
fake_bin="$test_root/bin"
mkdir -p "$fake_bin"

git init --quiet --bare "$remote"
git clone --quiet "$remote" "$work"
git -C "$work" switch --quiet -c main
git -C "$work" config user.email test@example.com
git -C "$work" config user.name "Release Plan Test"
printf '[package]\nname = "agent-gov"\nversion = "0.1.0"\n' > "$work/Cargo.toml"
git -C "$work" add Cargo.toml
git -C "$work" commit --quiet -m initial
git -C "$work" push --quiet -u origin main
git --git-dir="$remote" symbolic-ref HEAD refs/heads/main
git -C "$work" tag v0.1.0
git -C "$work" push --quiet origin v0.1.0

printf '%s\n' \
  '#!/usr/bin/env bash' \
  '[[ "${FAKE_RELEASE_EXISTS:-false}" == "true" ]]' > "$fake_bin/gh"
chmod 0755 "$fake_bin/gh"

assert_output() {
  local output="$1"
  local expected="$2"
  grep -Fqx "$expected" "$output" || {
    printf 'missing release-plan output: %s\n' "$expected" >&2
    exit 1
  }
}

run_plan() {
  local output="$1"
  local event="$2"
  local requested="$3"
  local selected="$4"
  local release_exists="$5"
  : > "$output"
  (
    cd "$work"
    PATH="$fake_bin:$PATH" \
      FAKE_RELEASE_EXISTS="$release_exists" \
      GITHUB_OUTPUT="$output" \
      RELEASE_DEFAULT_BRANCH=main \
      RELEASE_EVENT_NAME="$event" \
      RELEASE_REQUESTED_TAG="$requested" \
      RELEASE_SELECTED_REF="$selected" \
      "$repo_root/scripts/release-plan.sh"
  )
}

tag_sha=$(git -C "$work" rev-list -n 1 v0.1.0)
output="$test_root/output"

run_plan "$output" workflow_run "" main true
assert_output "$output" "publish=false"
assert_output "$output" "source-sha=$tag_sha"
assert_output "$output" "tag=v0.1.0"

run_plan "$output" workflow_dispatch v0.1.0 main true
assert_output "$output" "publish=true"
assert_output "$output" "source-sha=$tag_sha"

if run_plan "$output" workflow_dispatch v9.9.9 main false >/dev/null 2>&1; then
  printf 'mismatched release tag was accepted\n' >&2
  exit 1
fi

if run_plan "$output" workflow_dispatch "" feature false >/dev/null 2>&1; then
  printf 'new release from a non-default branch was accepted\n' >&2
  exit 1
fi

printf '[package]\nname = "agent-gov"\nversion = "0.2.0-rc.1"\n' > "$work/Cargo.toml"
git -C "$work" add Cargo.toml
git -C "$work" commit --quiet -m prerelease
new_sha=$(git -C "$work" rev-parse HEAD)
run_plan "$output" workflow_run "" main false
assert_output "$output" "publish=true"
assert_output "$output" "source-sha=$new_sha"
assert_output "$output" "tag=v0.2.0-rc.1"
assert_output "$output" "prerelease=true"

printf 'release-plan tests passed\n'
