#!/usr/bin/env bash

set -Eeuo pipefail

readonly version="1.7.12"
temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/agent-gov-actionlint.XXXXXX")"
trap 'rm -rf "$temp_dir"' EXIT

case "$(uname -s)-$(uname -m)" in
  Darwin-x86_64)
    platform="darwin_amd64"
    expected="5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644"
    ;;
  Darwin-arm64)
    platform="darwin_arm64"
    expected="aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f"
    ;;
  Linux-x86_64)
    platform="linux_amd64"
    expected="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
    ;;
  Linux-aarch64 | Linux-arm64)
    platform="linux_arm64"
    expected="325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6"
    ;;
  *)
    printf 'unsupported actionlint platform: %s-%s\n' "$(uname -s)" "$(uname -m)" >&2
    exit 1
    ;;
esac

archive="actionlint_${version}_${platform}.tar.gz"
curl --fail --location --silent --show-error --retry 3 \
  --proto '=https' --tlsv1.2 \
  --output "$temp_dir/$archive" \
  "https://github.com/rhysd/actionlint/releases/download/v${version}/${archive}"

actual=$(shasum -a 256 "$temp_dir/$archive")
actual="${actual%% *}"
[[ "$actual" == "$expected" ]] || {
  printf 'actionlint checksum mismatch\n' >&2
  exit 1
}

tar --no-same-owner -xzf "$temp_dir/$archive" -C "$temp_dir" actionlint
"$temp_dir/actionlint" -color
