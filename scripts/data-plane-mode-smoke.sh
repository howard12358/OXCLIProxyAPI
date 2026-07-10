#!/usr/bin/env bash

set -euo pipefail

# Runs the same deployed stack twice. The caller supplies two complete configs so
# this script never edits auth material or attempts to rewrite YAML in place.
EMBEDDED_CONFIG="${EMBEDDED_CONFIG:?set EMBEDDED_CONFIG to a config with embedded mode}"
DISABLED_CONFIG="${DISABLED_CONFIG:?set DISABLED_CONFIG to a config with data-plane.mode: disabled}"
API_KEY="${API_KEY:?set API_KEY}"
MANAGEMENT_KEY="${MANAGEMENT_KEY:-${CPA_MANAGEMENT_KEY:-}}"
CONTAINER_NAME="${CONTAINER_NAME:-ox-cli-proxy-api}"

if [[ -z "${MANAGEMENT_KEY}" ]]; then
  echo "Error: MANAGEMENT_KEY or CPA_MANAGEMENT_KEY is required." >&2
  exit 1
fi

for command in docker; do
  command -v "${command}" >/dev/null 2>&1 || {
    echo "Error: required command not found: ${command}" >&2
    exit 1
  }
done

cleanup() {
  docker compose down --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

run_mode() {
  local name="$1"
  local config="$2"
  local executor="$3"
  local rust_logs="$4"

  [[ -f "${config}" ]] || {
    echo "Error: ${name} config does not exist: ${config}" >&2
    exit 1
  }

  docker compose down --remove-orphans
  CLI_PROXY_CONFIG_PATH="${config}" docker compose up -d
  docker compose logs --tail=200 cli-proxy-api

  CPA_BASE_URL="${CPA_BASE_URL:-http://127.0.0.1:8317}" \
  CONTAINER_NAME="${CONTAINER_NAME}" \
  API_KEY="${API_KEY}" \
  MANAGEMENT_KEY="${MANAGEMENT_KEY}" \
  EXPECTED_EXECUTOR="${executor}" \
  EXPECT_RUST_LOGS="${rust_logs}" \
  "$(dirname "$0")/embedded-smoke.sh"
}

run_mode "disabled" "${DISABLED_CONFIG}" "CodexExecutor" "false"
run_mode "embedded" "${EMBEDDED_CONFIG}" "RustResponsesExecutor" "true"
