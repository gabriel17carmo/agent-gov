#!/usr/bin/env bash

set -Eeuo pipefail

readonly REPOSITORY="gabriel17carmo/agent-gov"
readonly DEFAULT_BIN_DIR="${HOME}/.local/bin"

bin_dir="${AGENT_GOV_INSTALL_DIR:-$DEFAULT_BIN_DIR}"
version="${AGENT_GOV_VERSION:-latest}"
agents="claude,cursor"
with_rtk=0
rtk_path=""
install_hooks=1
temp_dir=""
staged_binary=""

say() {
  printf 'agent-gov installer: %s\n' "$*"
}

die() {
  printf 'agent-gov installer: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Install the latest Agent Governor release and configure its agent hooks.

Usage:
  install-agent-gov.sh [options]

Options:
  --agents LIST       Agents to configure (default: claude,cursor)
  --with-rtk          Compose with the rtk executable found on PATH
  --rtk PATH          Compose with an explicit rtk executable
  --bin-dir PATH      Install directory (default: $HOME/.local/bin)
  --version VERSION   Release version, with or without a leading v
  --no-hooks          Install the binary without changing agent settings
  -h, --help          Show this help

Environment equivalents:
  AGENT_GOV_INSTALL_DIR, AGENT_GOV_VERSION
EOF
}

cleanup() {
  if [[ -n "$staged_binary" && -e "$staged_binary" ]]; then
    rm -f "$staged_binary"
  fi
  if [[ -n "$temp_dir" && -d "$temp_dir" ]]; then
    rm -rf "$temp_dir"
  fi
}

require_value() {
  local option="$1"
  local remaining="$2"
  [[ "$remaining" -ge 2 ]] || die "$option requires a value"
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --agents)
      require_value "$1" "$#"
      agents="$2"
      shift 2
      ;;
    --with-rtk)
      with_rtk=1
      shift
      ;;
    --rtk)
      require_value "$1" "$#"
      with_rtk=1
      rtk_path="$2"
      shift 2
      ;;
    --bin-dir)
      require_value "$1" "$#"
      bin_dir="$2"
      shift 2
      ;;
    --version)
      require_value "$1" "$#"
      version="$2"
      shift 2
      ;;
    --no-hooks)
      install_hooks=0
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1 (try --help)"
      ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || die "only macOS is supported by the prebuilt release"
case "$(uname -m)" in
  arm64 | x86_64) ;;
  *) die "unsupported Mac architecture: $(uname -m)" ;;
esac

[[ "$bin_dir" == /* ]] || die "--bin-dir must be an absolute path"
[[ -n "$agents" ]] || die "--agents cannot be empty"
if [[ "$install_hooks" -eq 0 && "$with_rtk" -eq 1 ]]; then
  die "--with-rtk and --rtk cannot be combined with --no-hooks"
fi

if [[ "$version" != "latest" ]]; then
  [[ "$version" == v* ]] || version="v$version"
  case "$version" in
    *[!0-9A-Za-z._-]*) die "invalid version: $version" ;;
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) die "version must look like v0.1.0" ;;
  esac
fi

for command_name in curl shasum install mktemp; do
  command -v "$command_name" >/dev/null 2>&1 || die "required command not found: $command_name"
done

if [[ -n "$rtk_path" ]]; then
  [[ "$rtk_path" == /* ]] || die "--rtk must be an absolute path"
  [[ -x "$rtk_path" ]] || die "rtk is not executable: $rtk_path"
elif [[ "$with_rtk" -eq 1 ]]; then
  command -v rtk >/dev/null 2>&1 || die "--with-rtk requires rtk on PATH (or pass --rtk PATH)"
fi

if [[ "$version" == "latest" ]]; then
  release_url="https://github.com/${REPOSITORY}/releases/latest/download"
else
  release_url="https://github.com/${REPOSITORY}/releases/download/${version}"
fi

temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/agent-gov-install.XXXXXX")"
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

downloaded_binary="$temp_dir/agent-gov"
downloaded_checksum="$temp_dir/agent-gov.sha256"

say "downloading ${version} release"
curl --fail --location --silent --show-error --retry 3 \
  --proto '=https' --tlsv1.2 \
  --output "$downloaded_binary" "$release_url/agent-gov" \
  || die "release binary not found; see the source install in the README"
curl --fail --location --silent --show-error --retry 3 \
  --proto '=https' --tlsv1.2 \
  --output "$downloaded_checksum" "$release_url/agent-gov.sha256" \
  || die "release checksum not found"

read -r expected_checksum _ < "$downloaded_checksum" || die "cannot read release checksum"
checksum_output="$(shasum -a 256 "$downloaded_binary")"
actual_checksum="${checksum_output%% *}"
case "$expected_checksum" in
  *[!0-9a-fA-F]* | "") die "release checksum is malformed" ;;
esac
[[ "${#expected_checksum}" -eq 64 ]] || die "release checksum is malformed"
[[ "$actual_checksum" == "$expected_checksum" ]] || die "checksum verification failed; binary was not installed"

chmod 0755 "$downloaded_binary"
binary_version="$("$downloaded_binary" --version 2>/dev/null)" \
  || die "downloaded binary could not run on this Mac"
[[ "$binary_version" == agent-gov\ * ]] || die "downloaded file is not an agent-gov binary"
if [[ "$version" != "latest" && "$binary_version" != "agent-gov ${version#v}" ]]; then
  die "downloaded $binary_version, but $version was requested"
fi

mkdir -p "$bin_dir"
staged_binary="$(mktemp "$bin_dir/.agent-gov.install.XXXXXX")"
install -m 0755 "$downloaded_binary" "$staged_binary"
mv -f "$staged_binary" "$bin_dir/agent-gov"
staged_binary=""
say "installed $binary_version at $bin_dir/agent-gov"

if [[ "$install_hooks" -eq 1 ]]; then
  install_command=("$bin_dir/agent-gov" install --agents "$agents")
  if [[ "$with_rtk" -eq 1 ]]; then
    install_command+=(--with-rtk)
  fi
  if [[ -n "$rtk_path" ]]; then
    install_command+=(--rtk "$rtk_path")
  fi
  "${install_command[@]}"

  set +e
  "$bin_dir/agent-gov" doctor
  doctor_status=$?
  set -e
  if [[ "$doctor_status" -ge 2 ]]; then
    die "doctor found an installation error; run '$bin_dir/agent-gov doctor' for details"
  elif [[ "$doctor_status" -eq 1 ]]; then
    say "doctor completed with preview warnings"
  fi
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    say "$bin_dir is not on PATH"
    say "add this line to your shell profile: export PATH=\"$bin_dir:\$PATH\""
    ;;
esac

say "done"
