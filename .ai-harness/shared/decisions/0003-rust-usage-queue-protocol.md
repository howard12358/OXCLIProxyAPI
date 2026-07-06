# 0003-rust-usage-queue-protocol.md

## Status

Accepted

## Context

CPA 的 usage 消费面不是独立 Prometheus 指标接口，而是进程内 redisqueue 加上两类消费协议：

- HTTP 管理接口：`GET /v0/management/usage-queue?count=N`
- Redis RESP 兼容协议：`AUTH`、`SUBSCRIBE usage/errors`、`LPOP/RPOP usage`

`cpa-usage-keeper` 优先通过 `REDIS_QUEUE_ADDR` 长连接订阅 `usage`，失败后才降级到 pop 或 HTTP 管理接口。Rust 数据平面如果只记录日志或只暴露 HTTP，就不能对齐 CPA 的实际运维消费路径。

## Decision

Rust 数据平面实现 CPA-compatible usage queue：

- `/v1/responses` telemetry 在 snapshot 开启 `usage_queue.enabled=true` 且 `backend=redis` 时，把 CPA-shaped payload 写入本地 usage queue。
- Go snapshot 为 auth record 导出 `usage_source`，Rust 在 usage payload 中作为 CPA `source` 发出；Rust HTTP 入口从下游认证请求头恢复 CPA `api_key` 归因。
- 本地 usage queue 遵循 CPA 语义：有 `usage` 订阅者时直接广播，不再写入内存队列；没有订阅者时进入 FIFO 队列，供 pop 消费。
- `SubscribeUsage` 等价路径首包发送 `{"support_refresh":true}`，并保留 `refresh` 控制消息能力。
- HTTP 暴露 `/v0/management/usage-queue?count=N`，pop 后即消费。
- Redis RESP 兼容协议与 HTTP 共享同一个 TCP listener，通过首字节识别 RESP 前缀后分流到 RESP handler，其余连接继续进入 Axum HTTP。
- RESP `AUTH` 在 Rust 配置了 `snapshot_bearer_token` 时校验该 token；未配置时只保留 AUTH 命令语义，便于本地文件 snapshot 模式运行。
- Go CPA 在 data-plane base URL 有效且 usage 统计开启时，优先通过 Redis RESP `AUTH` + `SUBSCRIBE usage` 订阅 Rust usage，并把收到的 payload 原样写回 CPA `internal/redisqueue`，使外部消费者无需改连接地址。
- 如果 RESP 订阅不可用或断开，Go CPA 会先通过 Rust `/v0/management/usage-queue?count=64` 做一次 HTTP pop 兜底，然后延迟重连 RESP。

## Consequences

优点：

- `cpa-usage-keeper` 可以沿用 CPA 的优先消费路径。
- 外部部署可以继续让 `REDIS_QUEUE_ADDR` 指向 CPA，而不是直接指向 Rust 数据平面。
- Rust usage 输出不再只是日志 sink，而是具备可消费的协议面。
- 同端口分流保持 CPA 的部署习惯，不需要额外暴露一个 Redis-like 端口。

代价与风险：

- Rust 启动流程从纯 `axum::serve` 变成手写 TCP accept loop，需要持续覆盖 HTTP 与 RESP 两类连接。
- 当前 RESP 只实现 Keeper/CPA usage 所需的最小命令集，不是完整 Redis server。
- Home 模式 `LPUSH usage` 转发仍未实现，后续需要独立补齐和验证。
