#!/usr/bin/env bash

set -Eeuo pipefail

: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
: "${RELEASE_DEFAULT_BRANCH:?RELEASE_DEFAULT_BRANCH is required}"
: "${RELEASE_EVENT_NAME:?RELEASE_EVENT_NAME is required}"

readonly requested_tag="${RELEASE_REQUESTED_TAG:-}"
readonly selected_ref="${RELEASE_SELECTED_REF:-}"

version=$(awk '
  $0 == "[package]" { in_package = 1; next }
  in_package && /^\[/ { exit }
  in_package && /^version = "/ {
    sub(/^version = "/, "")
    sub(/"$/, "")
    print
    exit
  }
' Cargo.toml)

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
  printf 'Cargo.toml package version is not valid SemVer: %s\n' "$version" >&2
  exit 1
fi

tag="v$version"
prerelease=false
if [[ "$version" == *-* ]]; then
  prerelease=true
fi

if [[ -n "$requested_tag" && "$requested_tag" != "$tag" ]]; then
  printf 'Requested tag %s does not match Cargo.toml version %s\n' \
    "$requested_tag" "$version" >&2
  exit 1
fi

if [[ "$RELEASE_EVENT_NAME" == "workflow_dispatch" && -z "$requested_tag" \
  && "$selected_ref" != "$RELEASE_DEFAULT_BRANCH" ]]; then
  printf 'New releases must be dispatched from %s\n' "$RELEASE_DEFAULT_BRANCH" >&2
  exit 1
fi

source_sha=$(git rev-parse HEAD)
tag_exists=false
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
  tag_exists=true
  git fetch --force --no-tags origin "refs/tags/$tag:refs/tags/$tag"
  source_sha=$(git rev-list -n 1 "$tag")
fi

release_exists=false
if gh release view "$tag" >/dev/null 2>&1; then
  release_exists=true
fi

publish=true
if [[ "$RELEASE_EVENT_NAME" == "workflow_run" && "$release_exists" == "true" ]]; then
  publish=false
fi

{
  printf 'publish=%s\n' "$publish"
  printf 'source-sha=%s\n' "$source_sha"
  printf 'tag=%s\n' "$tag"
  printf 'tag-exists=%s\n' "$tag_exists"
  printf 'prerelease=%s\n' "$prerelease"
  printf 'version=%s\n' "$version"
} >> "$GITHUB_OUTPUT"

if [[ "$publish" == "true" ]]; then
  printf 'Publishing %s from %s\n' "$tag" "$source_sha"
else
  printf '%s is already published; nothing to do\n' "$tag"
fi
