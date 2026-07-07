# Responses 到 Codex 兼容矩阵

本文档只描述 **Rust 数据平面 `POST /v1/responses` 在 `provider=Codex` 时的请求发射边界**。

目标不是定义通用 OpenAI Responses 规范，而是明确：

- 哪些字段应透传
- 哪些字段应强制改写
- 哪些字段应过滤
- 哪些字段应按条件处理

事实来源：

1. Go 原生 CPA `internal/translator/codex/openai/responses/codex_openai-responses_request.go`
2. Rust `rust/cliproxy-data-plane/src/responses/protocol.rs`
3. 对应单元测试和 route 测试

如果 Go 原生 Codex translator 行为发生变化，应同步更新 Rust 发射逻辑和本文档。

## 1. 适用范围

只适用于：

- 下游入口：Rust `POST /v1/responses`
- 上游 provider：`Codex`

不适用于：

- Go 原生 `POST /v1/responses`
- `GET /v1/responses` WebSocket
- `POST /v1/responses/compact`
- 非 Codex provider

## 2. 字段矩阵

| 字段 / 语义 | Rust 当前策略 | 分类 | 说明 |
| --- | --- | --- | --- |
| `model` | 改写为 execution plan resolved model | 强制改写 | 使用 router-core 选出的上游模型名，而不是原始下游别名。 |
| `stream` | 保持下游值；非流式下游在特定聚合路径仍会上游流式执行 | 条件处理 | `execute_real_upstream()` 会为 Codex 非流式下游启用上游流式聚合，但 request emit 本身保留语义输入。 |
| `input=string` | 提升为 Responses message array | 强制改写 | 兼容旧式纯文本输入。 |
| `input=array/object` | 原样保留 | 透传 | 尤其要保留 Codex CLI 原生 Responses array input。 |
| `input[*].role=system` | 改写为 `developer` | 强制改写 | Codex upstream 不接受 `system` role。 |
| `instructions` | 为空时补默认 Codex instructions | 条件处理 | 非空时保留。 |
| `metadata` | 删除 | 过滤 | Codex upstream 当前不接收该字段。 |
| `store` | 强制设为 `false` | 强制改写 | 与 Go 原生 CPA 保持一致。 |
| `parallel_tool_calls` | 强制设为 `true` | 强制改写 | 与 Go 原生 CPA 保持一致。 |
| `include` | 强制设为 `["reasoning.encrypted_content"]` | 强制改写 | 与 Go 原生 CPA 保持一致。 |
| `max_output_tokens` | 删除 | 过滤 | 当前 Codex upstream 会报 400。 |
| `max_completion_tokens` | 删除 | 过滤 | 与 Go 原生 CPA 保持一致。 |
| `temperature` | 删除 | 过滤 | 与 Go 原生 CPA 保持一致。 |
| `top_p` | 删除 | 过滤 | 与 Go 原生 CPA 保持一致。 |
| `service_tier=priority` | 保留 | 条件处理 | 当前只保留 `priority`。 |
| `service_tier!=priority` | 删除 | 条件处理 | 与 Go 原生 CPA 保持一致。 |
| `truncation` | 删除 | 过滤 | Codex upstream 当前不接受。 |
| `context_management` | 删除 | 过滤 | Codex upstream 当前不接受。 |
| `user` | 删除 | 过滤 | Codex upstream 当前不接受。 |
| `tools[*].type=web_search_preview*` | 归一化为 `web_search` | 条件处理 | 兼容 Codex builtin tool 旧别名。 |
| `tool_choice.type=web_search_preview*` | 归一化为 `web_search` | 条件处理 | 顶层 tool choice alias 兼容。 |
| `tool_choice.tools[*].type=web_search_preview*` | 归一化为 `web_search` | 条件处理 | 嵌套 allowed tools alias 兼容。 |
| 其他未知顶层字段 | 当前默认透传 | 透传 | 但前提是没有被上表显式过滤或改写。 |

## 3. 当前边界原则

### 3.1 透传优先，但不是盲透传

Rust 数据平面现在会保留未知顶层字段，这对兼容 Codex CLI 新请求形状是必要的。

但对 Codex upstream 来说，**“保留未知字段”不是绝对原则**。只要 Go 原生 CPA 已经证明某些字段会触发上游拒绝，Rust 就必须在 request emit 边界先做兼容过滤或改写。

### 3.2 兼容逻辑收口在 request emit 边界

这些 Codex 特有兼容动作当前统一放在：

- `rust/cliproxy-data-plane/src/responses/protocol.rs`

原因：

- 这里最接近“下游语义 -> 上游请求形状”转换边界
- 不把 provider 兼容逻辑散落到 handler、telemetry 或 upstream transport 层
- route test 可以直接锁住最终发出的实际 payload

### 3.3 Rust 当前仍以 Go 原生 CPA 为兼容事实源

对 Codex 来说，当前最稳妥的策略不是自行发明一套“更通用”的请求规则，而是：

- 先对齐 Go 原生 CPA 已经验证过的兼容行为
- 再逐步扩展 Rust 特定能力

## 4. 当前测试覆盖

已覆盖的关键回归：

- `responses::tests::request_ir_preserves_codex_native_input_and_extra_fields`
- `responses::tests::normalize_upstream_request_strips_codex_unsupported_generation_fields`
- `responses::tests::normalize_upstream_request_applies_codex_compatibility_defaults_and_rewrites`
- `responses_route_preserves_codex_native_input_and_extra_fields`
- `responses_route_strips_codex_unsupported_generation_fields`
- `responses_route_applies_codex_compatibility_rewrites`

其中 route 测试会检查 mock upstream 实际收到的 `received_payload`，这是当前防止“内部看起来对，实际转发体不对”的最重要保护。

## 5. 后续扩展规则

当新增或修改 Codex 兼容字段时，推荐顺序：

1. 先检查 Go 原生 CPA Codex translator 是否已有对应行为
2. 先写 Rust 单元测试，锁定 request emit 语义
3. 再写 Rust route 测试，锁定实际 forwarded payload
4. 最后修改 `protocol.rs`

如果未来 Go 与 Rust 的 Codex 兼容边界有意分叉，应先更新本文档和 `.ai-harness/shared/current-state.md`，不能静默漂移。
