#!/usr/bin/env bash

set -euo pipefail

CPA_BASE_URL="${CPA_BASE_URL:-http://127.0.0.1:8317}"
KEEPER_URL="${KEEPER_URL:-}"
CONTAINER_NAME="${CONTAINER_NAME:-ox-cli-proxy-api}"
MODEL="${MODEL:-gpt-5.5}"
MANAGEMENT_KEY="${MANAGEMENT_KEY:-${CPA_MANAGEMENT_KEY:-}}"
API_KEY="${API_KEY:-}"
CURL_MAX_TIME="${CURL_MAX_TIME:-90}"
EXPECTED_EXECUTOR="${EXPECTED_EXECUTOR:-RustResponsesExecutor}"
EXPECT_RUST_LOGS="${EXPECT_RUST_LOGS:-true}"

if [[ -z "${MANAGEMENT_KEY}" ]]; then
  echo "Error: MANAGEMENT_KEY or CPA_MANAGEMENT_KEY is required." >&2
  exit 1
fi

if [[ -z "${API_KEY}" ]]; then
  echo "Error: API_KEY is required." >&2
  exit 1
fi

for cmd in curl grep docker jq; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "Error: required command not found: ${cmd}" >&2
    exit 1
  fi
done

TMP_DIR="$(mktemp -d)"
dump_diagnostics() {
  local status="$1"
  if [[ "${status}" -eq 0 ]]; then
    rm -rf "${TMP_DIR}"
    return
  fi

  echo "---- embedded smoke diagnostics ----" >&2
  for file in "${TMP_DIR}"/*; do
    [[ -f "${file}" ]] || continue
    echo "---- $(basename "${file}") ----" >&2
    cat "${file}" >&2
  done
  docker logs --tail 400 "${CONTAINER_NAME}" >&2 || true
  echo "------------------------------------" >&2
  rm -rf "${TMP_DIR}"
}
trap 'status=$?; dump_diagnostics "${status}"' EXIT

PASS_COUNT=0

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  echo "[PASS] $1"
}

fail() {
  echo "[FAIL] $1" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  local context="$3"
  if ! grep -q --fixed-strings "${pattern}" "${file}"; then
    echo "---- ${context} body ----" >&2
    cat "${file}" >&2
    echo "-------------------------" >&2
    fail "${context}: expected pattern not found: ${pattern}"
  fi
}

http_get() {
  local url="$1"
  local body_file="$2"
  local header_file="$3"
  curl -sS \
    --max-time "${CURL_MAX_TIME}" \
    -D "${header_file}" \
    -o "${body_file}" \
    -w '%{http_code}' \
    "${url}"
}

http_get_auth() {
  local url="$1"
  local body_file="$2"
  local header_file="$3"
  curl -sS \
    --max-time "${CURL_MAX_TIME}" \
    -H "Authorization: Bearer ${MANAGEMENT_KEY}" \
    -D "${header_file}" \
    -o "${body_file}" \
    -w '%{http_code}' \
    "${url}"
}

http_post_json() {
  local url="$1"
  local payload="$2"
  local body_file="$3"
  local header_file="$4"
  curl -sS \
    --max-time "${CURL_MAX_TIME}" \
    -H "Authorization: Bearer ${API_KEY}" \
    -H 'Content-Type: application/json' \
    -d "${payload}" \
    -D "${header_file}" \
    -o "${body_file}" \
    -w '%{http_code}' \
    "${url}"
}

check_healthz() {
  local body="${TMP_DIR}/healthz.body"
  local header="${TMP_DIR}/healthz.headers"
  local status
  status="$(http_get "${CPA_BASE_URL}/healthz" "${body}" "${header}")"
  [[ "${status}" == "200" ]] || fail "healthz returned ${status}"
  assert_contains "${body}" '"status":"ok"' "healthz"
  pass "healthz"
}

check_runtime_snapshot() {
  local body="${TMP_DIR}/snapshot.body"
  local header="${TMP_DIR}/snapshot.headers"
  local status
  status="$(http_get_auth "${CPA_BASE_URL}/v0/management/runtime-snapshot" "${body}" "${header}")"
  [[ "${status}" == "200" ]] || fail "runtime snapshot returned ${status}"
  jq -e '.routes.responses == true and (.version | type == "string") and (.generated_at | type == "string")' "${body}" >/dev/null || fail "runtime snapshot has unexpected schema"
  pass "runtime snapshot"
}

check_non_stream_responses() {
  local body="${TMP_DIR}/responses-non-stream.body"
  local header="${TMP_DIR}/responses-non-stream.headers"
  local payload
  local status
  payload="$(printf '{"model":"%s","input":"reply with exactly OK","reasoning":{"effort":"medium"}}' "${MODEL}")"
  status="$(http_post_json "${CPA_BASE_URL}/v1/responses" "${payload}" "${body}" "${header}")"
  [[ "${status}" == "200" ]] || fail "non-stream responses returned ${status}"
  jq -e '.object == "response" and .status == "completed" and (.usage | type == "object")' "${body}" >/dev/null || fail "non-stream response has unexpected schema"
  pass "non-stream responses"
}

check_stream_responses() {
  local body="${TMP_DIR}/responses-stream.body"
  local header="${TMP_DIR}/responses-stream.headers"
  local payload
  local status
  payload="$(printf '{"model":"%s","input":"count to 3, one number per line","stream":true,"reasoning":{"effort":"low"}}' "${MODEL}")"
  status="$(http_post_json "${CPA_BASE_URL}/v1/responses" "${payload}" "${body}" "${header}")"
  [[ "${status}" == "200" ]] || fail "stream responses returned ${status}"
  grep -qi 'content-type: text/event-stream' "${header}" || fail "stream responses missing SSE content type"
  assert_contains "${body}" 'event: response.created' "stream responses body"
  assert_contains "${body}" 'event: response.completed' "stream responses body"
  assert_contains "${body}" '"status":"completed"' "stream responses body"
  pass "stream responses"
}

check_usage_queue_pop() {
  local body="${TMP_DIR}/usage-queue.body"
  local header="${TMP_DIR}/usage-queue.headers"
  local status
  local attempt

  for attempt in 1 2 3 4 5; do
    status="$(http_get_auth "${CPA_BASE_URL}/v0/management/usage-queue?count=4" "${body}" "${header}")"
    [[ "${status}" == "200" ]] || fail "usage queue pop returned ${status}"
    if jq -e --arg executor "${EXPECTED_EXECUTOR}" '
      type == "array" and any(.[]; .executor_type == $executor and .endpoint == "POST /v1/responses" and .failed == false)
    ' "${body}" >/dev/null; then
      pass "usage queue pop"
      return 0
    fi
    sleep 1
  done

  echo "---- usage queue body ----" >&2
  cat "${body}" >&2
  echo "-------------------------" >&2
  fail "usage queue pop returned no request records after smoke traffic"
}

check_keeper_reachability() {
  if [[ -z "${KEEPER_URL}" ]]; then
    return
  fi
  local body="${TMP_DIR}/keeper.body"
  local header="${TMP_DIR}/keeper.headers"
  local status
  status="$(http_get "${KEEPER_URL}/" "${body}" "${header}")"
  [[ "${status}" == "200" ]] || fail "keeper reachability returned ${status}"
  if ! grep -Eq '<!doctype html|<html' "${body}"; then
    echo "---- keeper body ----" >&2
    cat "${body}" >&2
    echo "---------------------" >&2
    fail "keeper reachability did not return HTML"
  fi
  pass "keeper reachability"
}

check_container_log_prefix() {
  local body="${TMP_DIR}/docker-logs.body"
  docker logs --tail 2000 "${CONTAINER_NAME}" >"${body}" 2>&1 || fail "docker logs failed for ${CONTAINER_NAME}"
  if [[ "${EXPECT_RUST_LOGS}" == "false" ]]; then
    if grep -Eq '\[rs-(stdout|stderr)\]' "${body}"; then
      fail "Rust child logs found while Go-native mode was expected"
    fi
    pass "Go-native mode has no Rust child logs"
    return
  fi
  if ! grep -Eq '\[rs-(stdout|stderr)\]' "${body}"; then
    echo "---- docker logs tail ----" >&2
    tail -n 120 "${body}" >&2 || true
    echo "--------------------------" >&2
    fail "rs log prefix not found in docker logs"
  fi
  pass "container rs log prefix"
}

check_healthz
check_runtime_snapshot
check_non_stream_responses
check_stream_responses
check_usage_queue_pop
check_keeper_reachability
check_container_log_prefix

echo "Data-plane smoke checks passed: ${PASS_COUNT} (executor=${EXPECTED_EXECUTOR}, rust_logs=${EXPECT_RUST_LOGS})"
