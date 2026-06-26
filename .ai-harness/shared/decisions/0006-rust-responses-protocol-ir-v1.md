# 0006-rust-responses-protocol-ir-v1.md

## Status

Accepted

## Context

Rust 数据平面的 `/v1/responses` 主链路已经具备可运行的最小协议转换能力，但在这次变更前，这些转换主要是隐式散落在两个位置：

- `src/responses/upstream.rs`
  - 直接把下游 OpenAI Responses 请求改写成 Codex 上游请求
- `src/responses/sse.rs`
  - 直接从 SSE frame 内部按 JSON `type` 分支处理 `response.output_item.done` 和 `response.completed`

这带来几个问题：

- 里程碑 5 所说的 request / stream event canonical IR 没有显式边界
- 请求改写与 SSE 修复都依赖 ad hoc JSON 访问，后续扩展 tool call / reasoning / usage 语义时风险更高
- 测试虽然覆盖了行为，但没有直接锁定“解析成什么 IR、再发射成什么协议”的中间契约

当前用户目标仍然是最小范围推进里程碑 5，而不是一次性引入新的 crate、目录层级或更大范围重构。

## Decision

在 `rust/cliproxy-data-plane/src/responses/` 内增加最小显式协议转换模块：

- `src/responses/protocol.rs`
  - `ResponsesRequestIr`
    - 承接下游 `/v1/responses` 请求的最小 canonical request IR
    - 负责发射到当前上游请求形状，先覆盖 OpenAI Responses -> Codex 请求适配
  - `ResponsesStreamEventIr`
    - 承接 SSE frame 的最小 canonical stream event IR
    - 先显式覆盖：
      - `response.output_item.done`
      - `response.completed`
      - 其他 JSON event
      - `[DONE]`
      - 非 JSON data payload

接入策略保持最小化：

- `upstream.rs`
  - 不再直接做 Codex 请求归一化细节，而是通过 `ResponsesRequestIr` 发射上游请求
- `sse.rs`
  - 不再直接按原始 JSON `type` 分支，而是先解析成 `ResponsesStreamEventIr` 再做 completed repair

这次不做：

- 不新增独立 `protocol-translate` crate
- 不改对外 HTTP API
- 不把全部 telemetry / usage / tool call 语义都抽进 IR

## Consequences

优点：

- 里程碑 5 从“只有隐式最小适配”提升为“已有显式最小 IR 边界”
- 请求发射和流事件解析有了可单测的中间层，后续扩 reasoning / usage / tool call 更顺
- 继续保持 `/v1/responses` 垂直切片内聚，不引入额外 crate 和跨目录震荡

代价与风险：

- 当前 IR 仍然只覆盖 `/v1/responses` 的 MVP 子集，不是通用协议转换框架
- canonical response IR 仍未完整独立建模，部分 completed/output 修复逻辑仍保留在 `sse.rs`
- 如果后续继续扩更多 provider 或更多下游协议，仍可能需要把该模块再提升成独立 crate

## Alternatives Considered

- 直接新增 `crates/protocol-translate/`
  - rejected，因为这会引入更大的目录结构和架构调整，当前需求只要求最小闭环
- 继续保留隐式 JSON 改写，不建立显式 IR
  - rejected，因为里程碑 5 的主要缺口正是中间语义边界不清
