# 0005-rust-observability-minimum.md

## Status

Superseded by `0003-rust-usage-queue-protocol.md`

## Context

Rust 数据平面已经具备 `/v1/responses` 的 MVP 主链路，但在切流前仍缺少最基本的运维闭环：

- usage 事件异步输出
- 与 CPA 现有 usage queue 语义对齐的最小热路径注入点

如果没有这些能力，即使主链路功能可用，也难以安全判断：

- usage 数据是否能从 Rust 主链路异步产出
- Rust 产出的 usage 结构能否直接对齐 CPA 后续消费链路

## Decision

先落地里程碑 8 的最小 CPA 对齐子集，不等待完整事件平台：

- 新增 `src/telemetry.rs`
  - 统一请求生命周期观测、usage 提取与异步事件投递
- 新增 `crates/usage-events`
  - 定义 CPA usage queue 兼容 payload
  - 提供异步 producer
  - 首版只提供 `log` sink 和 `noop` 行为
- 在 `/v1/responses` 主链路接入最小 telemetry：
  - 提取 usage
  - 记录首字节与总耗时到 payload
  - 产出与 CPA `internal/redisqueue/plugin.go` 同形的 usage payload
  - 仅在 `usage_queue.enabled=true` 且 `usage_queue.backend=redis` 时发射

首版范围刻意限制为：

- 只覆盖 Rust `POST /v1/responses`
- 只覆盖当前 OpenAI / Codex responses 主链路
- 不额外引入 Prometheus 路由
- 不在本次变更里补完整的 CPA redis 订阅 / pop 协议入口

## Consequences

优点：

- 在不扩大 provider 范围的前提下，先把 Rust usage 输出语义拉回 CPA 现状
- usage payload 输出异步化，不阻塞热路径
- 为后续完整的里程碑 8 演进保留清晰落点

代价与风险：

- 首版 sink 只有日志输出，后续已由 `0003-rust-usage-queue-protocol.md` 补为本地 usage queue 与 HTTP/RESP 消费协议
- 当前 snapshot 不包含 CPA usage payload 所有细粒度字段，部分字段只能用现有可得信息回填或留空
- Home 模式 `LPUSH usage` 转发和 errors 通道生产来源仍未补齐

## Alternatives Considered

- 先做 Rust `/metrics` + Prometheus
  - rejected，因为偏离 CPA 当前 usage 集成方式
- 直接在本次变更里复制完整 CPA redis 协议服务面
  - rejected，因为超出“最小 CPA 对齐闭环”的当前范围
