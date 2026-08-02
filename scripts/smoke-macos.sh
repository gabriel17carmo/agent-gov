#!/bin/sh
set -eu

test "$(uname -s)" = "Darwin"
test_home="$(mktemp -d)"
trap 'rm -rf "$test_home"' EXIT INT TERM

export AGENT_GOV_TEST_HOME="$test_home/state"
export AGENT_GOV_TEST_USER_HOME="$test_home/user"

cargo build --locked
binary="$PWD/target/debug/agent-gov"
"$binary" classify -- "npm test" | grep 'rewrite: yes'
"$binary" run --owner smoke -- /usr/bin/true
"$binary" status --json | grep '"capacity": 1'
"$binary" doctor --json >/dev/null || test "$?" -eq 1
