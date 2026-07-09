# Mock Upstream Baseline Benchmark

This runbook describes how to benchmark the embedded Rust `/v1/responses` data plane
against the Go native path using a mock upstream (no real AI provider).

> **Status**: documented, not yet executed.

## Prerequisites

- Docker Engine with `docker compose` (v2)
- `curl`, `wrk` or equivalent HTTP benchmarking tool
- A mock upstream server that returns predictable JSON/SSE

## Comparison Groups

1. **Go native** `/v1/responses` (Rust data-plane disabled)
2. **Embedded Rust** `/v1/responses` (default on `rusty` branch)
3. **Standalone Rust** data-plane (if applicable)

## Mock Upstream Behavior

### Non-Streaming
```
sleep 100ms
return fixed JSON: {"id":"resp_bench_001","object":"response","status":"completed",...}
```

### Streaming
```
every 50ms emit one SSE frame
total 100 frames
then event: response.completed + [DONE]
```

The mock upstream should be a separate container or process that does NOT introduce
meaningful latency variance between the two paths.

## Metrics

| Metric | Description |
| ------ | ----------- |
| RSS | Resident set size after warm-up |
| CPU | CPU seconds per 1000 requests |
| P50 latency | Median end-to-end response time |
| P95 latency | 95th percentile |
| P99 latency | 99th percentile |
| Stream concurrency | Max concurrent SSE connections before failure |
| Chunk forwarding latency | Time from upstream emit to client receive |
| Client abort → upstream release | Time from client drop to upstream write error |
| Usage queue write latency | Time from response end to usage payload in queue |
| Error rate | % of requests returning non-2xx |

## Test Scenarios

### 1. Non-Stream Throughput
```bash
wrk -t4 -c100 -d30s --latency \
  -s scripts/bench/non-stream.lua \
  http://127.0.0.1:8317/v1/responses
```

### 2. Stream Concurrency
```bash
# Launch N concurrent curl processes, each holding an SSE stream for 30s
for i in $(seq 1 50); do
  curl -N http://127.0.0.1:8317/v1/responses ... &
done
wait
```

### 3. Client Abort Release Time
```bash
# Start stream, read 2 frames, drop, measure time until upstream sees write error
```

## Disabled Mode vs Embedded Mode

To compare Go native vs Rust:

1. Set `data-plane.mode: disabled` → Go native path
2. Run benchmark → record metrics
3. Set `data-plane.mode: embedded` (or omit) → Rust path
4. Run benchmark → record metrics
5. Compare

## Prohibited

- Do NOT benchmark against real OpenAI / Codex / Gemini upstreams.
- Do NOT use production API keys in benchmark scripts.
- Do NOT run benchmarks against shared/staging deployments without explicit approval.

## Script Location

Benchmark scripts should live under `scripts/bench/` when implemented.

## Success Criteria (for Default Enable)

- Rust P50 latency within 20% of Go native
- Rust RSS does not exceed Go native by more than 2x
- Stream concurrency at least 100 connections without errors
- Client abort releases upstream within 1 second
- Zero error rate under mock upstream
