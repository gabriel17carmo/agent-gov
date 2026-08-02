#!/bin/sh
set -eu

cargo build --release --locked --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin
mkdir -p dist
lipo -create \
  target/aarch64-apple-darwin/release/agent-gov \
  target/x86_64-apple-darwin/release/agent-gov \
  -output dist/agent-gov
chmod 755 dist/agent-gov
file dist/agent-gov
