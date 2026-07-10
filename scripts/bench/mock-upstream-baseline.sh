#!/usr/bin/env bash

set -euo pipefail

# This runner deliberately requires an explicit acknowledgement so benchmark
# traffic cannot accidentally be directed at a real provider.
: "${MOCK_UPSTREAM_CONFIRMED:?set MOCK_UPSTREAM_CONFIRMED=1 after pointing the stack at a mock upstream}"
: "${BASE_URL:?set BASE_URL, for example http://127.0.0.1:8317}"
: "${API_KEY:?set API_KEY}"

MODE="${MODE:-embedded}"
MODEL="${MODEL:-gpt-5-codex}"
REQUESTS="${REQUESTS:-20}"
CONCURRENCY="${CONCURRENCY:-1}"
SCENARIOS="${SCENARIOS:-nonstream-small,nonstream-large,stream-short,stream-long,stream-abort}"
OUT_DIR="${OUT_DIR:-artifacts/bench/${MODE}-$(date -u +%Y%m%dT%H%M%SZ)}"
CONTAINER_NAME="${CONTAINER_NAME:-ox-cli-proxy-api}"

if [[ "${MOCK_UPSTREAM_CONFIRMED}" != "1" ]]; then
  echo "MOCK_UPSTREAM_CONFIRMED must be exactly 1." >&2
  exit 1
fi

for command in curl jq git uname; do
  command -v "${command}" >/dev/null 2>&1 || {
    echo "required command not found: ${command}" >&2
    exit 1
  }
done

mkdir -p "${OUT_DIR}"
samples="${OUT_DIR}/samples.jsonl"
: > "${samples}"

make_input() {
  local scenario="$1"
  case "${scenario}" in
    nonstream-small|stream-short|stream-abort) printf 'mock benchmark small request' ;;
    nonstream-large|stream-long) head -c 16384 < /dev/zero | tr '\0' 'x' ;;
    *) echo "unknown scenario: ${scenario}" >&2; return 1 ;;
  esac
}

is_stream() {
  [[ "$1" == stream-* ]]
}

run_one() {
  local scenario="$1"
  local index="$2"
  local input stream body output timing status
  input="$(make_input "${scenario}")"
  stream=false
  is_stream "${scenario}" && stream=true
  body="$(jq -cn --arg model "${MODEL}" --arg input "${input}" --argjson stream "${stream}" '{model:$model,input:$input,stream:$stream}')"
  output="${OUT_DIR}/${scenario}-${index}.body"

  # The abort case reads only briefly; its curl exit code is expected to be nonzero.
  if [[ "${scenario}" == "stream-abort" ]]; then
    timing="$(curl -sS -N --max-time 1 -o "${output}" -w '%{http_code} %{time_total}' \
      -H "Authorization: Bearer ${API_KEY}" -H 'Content-Type: application/json' \
      -d "${body}" "${BASE_URL}/v1/responses" || true)"
  else
    timing="$(curl -sS -N -o "${output}" -w '%{http_code} %{time_total}' \
      -H "Authorization: Bearer ${API_KEY}" -H 'Content-Type: application/json' \
      -d "${body}" "${BASE_URL}/v1/responses")"
  fi
  status="${timing%% *}"
  timing="${timing#* }"
  jq -cn --arg scenario "${scenario}" --argjson status "${status:-0}" --argjson seconds "${timing:-0}" \
    '{scenario:$scenario,status:$status,latency_ms:($seconds * 1000)}' >> "${samples}"
}

IFS=',' read -r -a scenario_list <<< "${SCENARIOS}"
for scenario in "${scenario_list[@]}"; do
  for index in $(seq 1 "${REQUESTS}"); do
    run_one "${scenario}" "${index}" &
    while [[ "$(jobs -pr | wc -l | tr -d ' ')" -ge "${CONCURRENCY}" ]]; do
      wait -n
    done
  done
  wait
done

docker_stats='{}'
if docker inspect "${CONTAINER_NAME}" >/dev/null 2>&1; then
  docker_stats="$(docker stats --no-stream --format '{{json .}}' "${CONTAINER_NAME}" | jq -R 'fromjson? // {}')"
fi

jq -s \
  --arg mode "${MODE}" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg platform "$(uname -srm)" \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson docker_stats "${docker_stats}" '
  def percentile($p): sort | .[((length - 1) * $p | floor)];
  {
    mode: $mode,
    commit: $commit,
    platform: $platform,
    generated_at: $generated_at,
    docker_stats: $docker_stats,
    scenarios: group_by(.scenario) | map({
      scenario: .[0].scenario,
      requests: length,
      error_rate: ([.[] | select(.status < 200 or .status >= 300)] | length / length),
      latency_ms: ([.[].latency_ms] | {p50: percentile(0.50), p95: percentile(0.95), p99: percentile(0.99)})
    })
  }
' "${samples}" > "${OUT_DIR}/results.json"

{
  echo '# Mock Upstream Benchmark Result'
  echo
  echo "- Mode: \`${MODE}\`"
  echo "- Commit: \`$(git rev-parse --short HEAD)\`"
  echo "- Platform: \`$(uname -srm)\`"
  echo
  echo '| Scenario | Requests | Error rate | P50 ms | P95 ms | P99 ms |'
  echo '| --- | ---: | ---: | ---: | ---: | ---: |'
  jq -r '.scenarios[] | "| \(.scenario) | \(.requests) | \(.error_rate) | \(.latency_ms.p50) | \(.latency_ms.p95) | \(.latency_ms.p99) |"' "${OUT_DIR}/results.json"
} > "${OUT_DIR}/summary.md"

echo "machine-readable result: ${OUT_DIR}/results.json"
echo "markdown summary: ${OUT_DIR}/summary.md"
