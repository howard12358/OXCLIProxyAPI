#!/usr/bin/env bash
set -euo pipefail

: "${RUST_TARGET:?RUST_TARGET is required}"
: "${RELEASE_VERSION:?RELEASE_VERSION is required}"

binary_name="cliproxy-data-plane"
binary_path="rust/cliproxy-data-plane/target/${RUST_TARGET}/release/${binary_name}"

case "${RUST_TARGET}" in
  *windows*)
    binary_name="${binary_name}.exe"
    binary_path="${binary_path}.exe"
    ;;
esac

cargo build --manifest-path rust/cliproxy-data-plane/Cargo.toml --release --locked --target "${RUST_TARGET}"

go run ./cmd/embed_data_plane \
  --source "${binary_path}" \
  --file-name "${binary_name}" \
  --version "${RELEASE_VERSION}"
