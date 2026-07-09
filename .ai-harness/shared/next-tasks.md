# OXCLIProxyAPI Rust 数据平面下一步任务文档

## 1. 当前阶段判断

当前项目已经完成了 Rust 数据平面的主要骨架建设，并且已经开始形成契约测试体系。

现阶段项目不再是单纯的“Rust data-plane 原型”，而是进入：

```text
Rust /v1/responses 生产化前夜
```

下一步工作重点不是继续盲目扩功能，而是把以下几个边界钉死：

```text
1. Codex Responses 兼容矩阵；
2. usage queue / Home 模式统计链路；
3. stream/client abort 资源释放语义；
4. Go / Rust snapshot 契约防漂移；
5. embedded 部署 smoke 验证。
```

核心目标：

```text
让 Rust 数据平面从“能跑”变成“敢默认启用”。
```

---

## 2. 总体优先级

| 优先级 | 任务                                                  | 目标                                      |
| --- | --------------------------------------------------- | --------------------------------------- |
| P0  | 整理测试代码可读性与测试命令                                      | 保证 Codex / 人类后续能稳定维护                    |
| P0  | 扩展 `/v1/responses` Codex request emission golden 矩阵 | 钉死 Codex 请求兼容边界                         |
| P0  | 补 snapshot negative fixtures                        | 防止 Go exporter / Rust common-types 协议漂移 |
| P1  | 实现 Home 模式 `LPUSH usage` 最小闭环                       | 对齐原 CPA usage 统计链路                      |
| P1  | 补“下游客户端断开”stream abort 测试                           | 防止 stream 长连接泄漏                         |
| P2  | 跑 embedded smoke 并沉淀 runbook                        | 验证默认 Docker/embedded 路径可用               |

---

# 3. 任务一：测试代码可读性整理与 testing.md 校准

## 3.1 背景

当前 contract test 已经有初步体系，但部分测试文件在 GitHub raw 视图里显示为一整行。这会导致：

```text
1. review 困难；
2. diff 困难；
3. Codex 后续修改困难；
4. 契约测试维护成本升高。
```

契约测试是 Rust 数据面后续演进的护栏，测试代码本身必须保持可读。

## 3.2 任务目标

```text
1. 所有 Rust contract test 文件格式正常；
2. cargo fmt / cargo test 能稳定执行；
3. .ai-harness/shared/testing.md 中记录的命令必须真实可跑；
4. Go side 测试命令路径不能和实际仓库不一致。
```

## 3.3 建议执行命令

```bash
cargo fmt --manifest-path rust/cliproxy-data-plane/Cargo.toml
cargo test --workspace --manifest-path rust/cliproxy-data-plane/Cargo.toml
```

检查 Go 测试路径：

```bash
find internal -name '*snapshot*test.go' -print
find sdk -name '*test.go' | grep -i responses
go test ./internal/... -count=1
go test ./sdk/... -count=1
```

如果 `.ai-harness/shared/testing.md` 中记录的命令跑不通，必须同步修正。

## 3.4 验收标准

```text
1. cargo fmt 后无格式变更；
2. cargo test --workspace 通过；
3. Go 测试命令和 testing.md 完全一致；
4. 不存在明显“一整行测试文件”；
5. AGENTS.md / testing.md 中的验证命令不误导后续 Agent。
```

---

# 4. 任务二：扩展 `/v1/responses` Codex request emission golden 矩阵

## 4.1 背景

当前 Rust 数据平面已经实现 `/v1/responses` 的 handler、protocol、upstream、SSE 分层。
但 `/v1/responses` 的兼容性不能只靠一个 basic fixture 证明。

Codex Responses 请求发射逻辑里有很多兼容边界：

```text
1. input 格式归一；
2. system role -> developer role；
3. reasoning / reasoning_effort；
4. service_tier；
5. tools / parallel_tool_calls；
6. include encrypted_content；
7. unsupported generation fields 剥离；
8. builtin tool alias normalization；
9. web_search_preview -> web_search；
10. empty instructions 默认值。
```

这些必须进入 golden test。

## 4.2 任务目标

新增一组 Codex request emission golden fixtures，覆盖 Rust 发送到 mock upstream 的最终请求体。

目标不是只验证 Rust 返回给客户端什么，而是验证：

```text
下游请求 -> Rust protocol normalize -> 上游 Codex Responses 请求
```

这一段是否稳定。

## 4.3 建议新增 fixtures

目录建议：

```text
testdata/contract/responses/request_emission/
  input_string.request.json
  input_string.expected_upstream.json

  input_messages_array.request.json
  input_messages_array.expected_upstream.json

  system_role_to_developer.request.json
  system_role_to_developer.expected_upstream.json

  reasoning_effort.request.json
  reasoning_effort.expected_upstream.json

  service_tier.request.json
  service_tier.expected_upstream.json

  tools_empty_parallel_removed.request.json
  tools_empty_parallel_removed.expected_upstream.json

  tools_non_empty_parallel_preserved.request.json
  tools_non_empty_parallel_preserved.expected_upstream.json

  include_encrypted_content_injected.request.json
  include_encrypted_content_injected.expected_upstream.json

  unsupported_generation_fields_removed.request.json
  unsupported_generation_fields_removed.expected_upstream.json

  web_search_preview_normalized.request.json
  web_search_preview_normalized.expected_upstream.json
```

## 4.4 重点用例

### Case 1：string input lifting

输入：

```json
{
  "model": "gpt-5-codex",
  "input": "hello",
  "stream": false
}
```

期望上游请求必须符合当前 Codex 兼容约定。

断言：

```text
1. input 不丢失；
2. model 已解析到目标上游模型；
3. stream 字段符合请求；
4. 默认 include / metadata / store 等字段按约定处理。
```

---

### Case 2：system role -> developer role

输入包含：

```json
{
  "role": "system",
  "content": "You are helpful."
}
```

期望上游请求中变成：

```json
{
  "role": "developer",
  "content": "You are helpful."
}
```

断言：

```text
1. 不再出现 role=system；
2. 内容不丢失；
3. 消息顺序不变。
```

---

### Case 3：无 tools 时移除 parallel_tool_calls

输入：

```json
{
  "model": "gpt-5-codex",
  "input": "hello",
  "tools": [],
  "parallel_tool_calls": true
}
```

期望：

```text
1. tools 为空时，parallel_tool_calls 不发送给 Codex；
2. 不产生无意义字段；
3. 不影响普通响应。
```

---

### Case 4：有 tools 时保留或按约定处理 parallel_tool_calls

输入：

```json
{
  "model": "gpt-5-codex",
  "input": "hello",
  "tools": [
    {
      "type": "function",
      "name": "lookup",
      "description": "lookup something"
    }
  ],
  "parallel_tool_calls": true
}
```

断言：

```text
1. tools 不丢失；
2. parallel_tool_calls 行为符合当前协议约定；
3. tool schema 不被错误改写。
```

---

### Case 5：剥离 Codex 不支持的 generation fields

输入包含：

```json
{
  "temperature": 0.8,
  "top_p": 0.9,
  "max_output_tokens": 1000
}
```

期望：

```text
1. Codex 不支持的字段不发送；
2. Rust 不报错；
3. 输出请求体和 golden 完全一致。
```

## 4.5 测试实现建议

在 mock upstream 中捕获实际收到的 JSON request body：

```text
client request
  -> Rust /v1/responses
  -> mock upstream
  -> capture upstream request JSON
  -> normalize
  -> compare expected_upstream.json
```

normalize 规则：

```text
1. 删除动态 id；
2. 删除 timestamp；
3. object key 排序；
4. null 和缺省字段按项目约定统一；
5. Authorization header 不写入 golden。
```

## 4.6 验收标准

```text
1. 至少覆盖 10 个 request emission fixture；
2. 每个 fixture 都比较 mock upstream 捕获到的请求体；
3. cargo test 通过；
4. 新增字段或协议变更必须显式更新 golden；
5. 不允许 silent drift。
```

---

# 5. 任务三：补 snapshot negative fixtures

## 5.1 背景

Go 管理面输出 runtime snapshot，Rust 数据面只读 snapshot。
这个边界是整个 Go/Rust 混合架构的核心。

现在需要补负例测试，避免 Go exporter 或 Rust common-types 结构变化后出现 silent break。

## 5.2 任务目标

补充 invalid snapshot fixtures，验证 Rust validate 能明确失败。

## 5.3 建议新增 fixtures

目录：

```text
testdata/contract/snapshot/
```

新增：

```text
runtime_snapshot.invalid_missing_version.json
runtime_snapshot.invalid_missing_generated_at.json
runtime_snapshot.invalid_missing_source_instance_id.json
runtime_snapshot.invalid_codex_missing_access_token.json
runtime_snapshot.invalid_empty_auth_index.json
runtime_snapshot.invalid_empty_model_alias_target.json
runtime_snapshot.invalid_provider_missing_model.json
runtime_snapshot.invalid_route_missing_target.json
```

## 5.4 断言要求

每个 invalid fixture 都必须断言：

```text
1. serde parse 可以按场景成功或失败；
2. validate_snapshot 必须失败；
3. 错误消息包含关键字段名；
4. 不允许 fallback 成 degraded-but-accepted；
5. 不允许 panic。
```

示例：

```rust
#[test]
fn rejects_codex_auth_missing_access_token() {
    let raw = include_str!("../../../testdata/contract/snapshot/runtime_snapshot.invalid_codex_missing_access_token.json");
    let snapshot: RuntimeSnapshot = serde_json::from_str(raw).unwrap();

    let err = validate_snapshot(&snapshot).unwrap_err();
    assert!(err.to_string().contains("access_token"));
}
```

## 5.5 验收标准

```text
1. 正例 golden 能 parse + validate；
2. 所有 invalid fixtures 都被拒绝；
3. 错误信息可定位字段；
4. Go exporter 结构变更时，Rust snapshot test 能第一时间失败。
```

---

# 6. 任务四：实现 Home 模式 LPUSH usage 最小闭环

## 6.1 背景

当前 usage queue 已经具备：

```text
1. 本地 FIFO queue；
2. HTTP pop；
3. RESP LPOP/RPOP；
4. RESP SUBSCRIBE usage/errors；
5. Go bridge RESP 优先 + HTTP fallback。
```

但 Home 模式还缺：

```text
Rust 直接向外部 Redis / queue 执行 LPUSH usage
```

这会影响和原 CPA usage 统计链路的完整对齐。

## 6.2 任务目标

实现最小可用的 outbound usage writer：

```text
Rust usage payload
  -> local usage queue
  -> optional external RESP LPUSH usage <payload>
```

注意：第一版只做最小闭环，不要引入复杂 Redis 生态能力。

## 6.3 配置来源

优先从 runtime snapshot 的 `usage_queue` 或 Rust config 中读取：

```text
1. enabled；
2. mode；
3. address；
4. password / token；
5. key name，默认 usage；
6. timeout；
7. retry/backoff 策略。
```

如果 snapshot 字段还不够，先补契约文档，再补 common-types。

## 6.4 行为语义

建议第一版语义：

```text
1. Rust 本地 queue 始终保留现有行为；
2. 如果配置了 external LPUSH，则额外异步推送；
3. external LPUSH 失败不能影响主请求响应；
4. external LPUSH 失败要进入 tracing log；
5. 是否进入 errors channel 需要明确；
6. 不做无限重试；
7. 不阻塞 stream 热路径。
```

## 6.5 测试要求

新增 fake RESP server contract test：

```text
1. Rust 发成功 /v1/responses；
2. fake RESP server 捕获 AUTH；
3. fake RESP server 捕获 LPUSH usage <payload>；
4. payload 是合法 JSON；
5. payload normalize 后和 golden 匹配；
6. fake RESP server 故障时，请求仍成功；
7. 故障时有明确日志或 error event。
```

## 6.6 验收标准

```text
1. Home 模式 LPUSH usage 可配置；
2. LPUSH payload 字段与本地 usage payload 一致；
3. LPUSH 失败不影响 /v1/responses 主链路；
4. 有 contract test；
5. README / .ai-harness/current-state.md 更新当前能力。
```

---

# 7. 任务五：补“下游客户端断开”stream abort 测试

## 7.1 背景

当前已有 stream abort 相关测试，但重点更偏向：

```text
upstream -> Rust 中途断开
```

还需要补最关键的生产场景：

```text
client -> Rust 中途断开
Rust -> upstream 是否跟着取消？
```

这是网关型数据平面的关键稳定性问题。

## 7.2 任务目标

验证：

```text
1. client 读取 1~2 个 SSE frame 后主动 close；
2. Rust 能取消或关闭 upstream stream；
3. mock upstream 能观察到连接关闭或 write error；
4. Rust 不发生 post-commit auth retry；
5. usage queue 不写 success completed；
6. 不 panic；
7. 不泄漏 task / connection。
```

## 7.3 测试方式建议

不要只用 `tower::ServiceExt::oneshot()`。
应该启动真实 HTTP listener，用真实 client 连接，然后中途 drop response body。

mock upstream 提供慢速无限 SSE：

```text
event: response.created
data: {...}

event: response.output_text.delta
data: {...}

每 100ms 发送一个 delta，持续发送。
```

测试流程：

```text
1. 启动 mock upstream；
2. 启动 Rust data-plane listener；
3. client POST /v1/responses stream=true；
4. client 读取前两个 SSE frame；
5. client 主动 drop response；
6. 等待 200ms ~ 1s；
7. 断言 mock upstream 观察到关闭；
8. 断言 usage queue 没有 success completed；
9. 断言没有 retry 第二账号。
```

## 7.4 验收标准

```text
1. 下游断开后 1s 内释放上游连接；
2. 不发生 post-commit auth retry；
3. 不误记成功 usage；
4. 重复执行不会造成连接数上涨；
5. 测试稳定，不依赖真实上游。
```

---

# 8. 任务六：embedded smoke 与 runbook 沉淀

## 8.1 背景

当前默认部署路径已经是 embedded Rust data-plane。
需要把“能构建、能启动、能通过 smoke”沉淀成固定流程。

## 8.2 任务目标

补充 embedded smoke runbook：

```text
1. 如何构建 embedded 镜像；
2. 如何启动 docker-compose；
3. 如何确认 Go 管理面正常；
4. 如何确认 Rust data-plane 正常；
5. 如何发 `/v1/responses` 非流式请求；
6. 如何发 stream 请求；
7. 如何查看 Rust data-plane 日志；
8. 如何验证 usage queue；
9. 如何回滚到 Go 原生路径。
```

## 8.3 建议新增文档

```text
.ai-harness/runbooks/embedded-rust-data-plane-smoke.md
```

## 8.4 smoke 命令示例

```bash
docker compose up -d
docker compose logs -f cli-proxy-api
```

健康检查：

```bash
curl http://127.0.0.1:8317/healthz
curl http://127.0.0.1:8317/readyz
```

非流式请求：

```bash
curl http://127.0.0.1:8317/v1/responses \
  -H "Authorization: Bearer $CPA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-codex",
    "input": "hello",
    "stream": false
  }'
```

流式请求：

```bash
curl -N http://127.0.0.1:8317/v1/responses \
  -H "Authorization: Bearer $CPA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-codex",
    "input": "hello",
    "stream": true
  }'
```

usage queue：

```bash
curl http://127.0.0.1:8317/v0/management/usage-queue?count=10
```

## 8.5 验收标准

```text
1. runbook 按步骤执行能启动服务；
2. /healthz 和 /readyz 正常；
3. /v1/responses stream=false 正常；
4. /v1/responses stream=true 正常；
5. usage queue 有 payload；
6. 失败时能定位日志；
7. current-state.md 更新 smoke 结果。
```

---

# 9. 不建议现在做的事情

当前阶段不建议立刻做：

```text
1. 全量重写 Claude / Gemini / Grok provider；
2. 大规模重构 Go 管理面；
3. 改动配置文件格式；
4. 引入复杂插件系统；
5. 做 Web UI 大改；
6. 追求性能压测极限；
7. 为了覆盖率写无意义单测。
```

原因：

```text
当前最关键的是让 Rust /v1/responses 生产化，而不是扩大不稳定面。
```

---

# 10. 推荐给 Codex 的任务提示词

可以直接复制给 Codex：

```text
你现在继续推进 OXCLIProxyAPI rusty 分支的 Rust 数据平面生产化工作。

请先阅读：
1. AGENTS.md
2. .ai-harness/shared/current-state.md
3. .ai-harness/shared/testing.md
4. rust/cliproxy-data-plane/README.md
5. rust/cliproxy-data-plane/tests/contract/*
6. testdata/contract/*

本轮不要扩展新 provider，不要重构 Go 管理面，不要修改业务配置格式。

本轮目标按顺序完成：

1. 整理 Rust contract test 文件格式，确保 cargo fmt / cargo test 能通过；
2. 校准 .ai-harness/shared/testing.md 中所有测试命令，确保命令真实可跑；
3. 扩展 /v1/responses Codex request emission golden 矩阵，覆盖 input、system->developer、reasoning、service_tier、tools、include、unsupported generation fields、web_search_preview normalization；
4. 补 snapshot negative fixtures，验证 Rust validate_snapshot 能拒绝坏 snapshot；
5. 如果前四项完成，再开始实现 Home 模式 LPUSH usage 最小闭环；
6. 更新 .ai-harness/shared/current-state.md，记录完成情况、剩余风险和测试命令。

要求：
- 不要编造不存在的能力；
- 每个行为变更必须有测试；
- golden 更新必须可 review；
- 不要让 Rust 数据面写回 Go snapshot；
- 不要破坏 embedded docker 部署路径；
- 最后输出修改文件列表、测试结果和剩余 TODO。
```

---

# 11. 完成后的阶段目标

完成本文档中的 P0/P1 任务后，项目应达到：

```text
1. Rust /v1/responses Codex 路径具备较稳定兼容边界；
2. Go snapshot -> Rust common-types 契约能防漂移；
3. usage 统计链路接近原 CPA Home 模式；
4. stream 长连接异常场景有测试护栏；
5. embedded 部署路径有可执行 runbook；
6. 后续扩展 provider 时不会破坏现有数据面核心。
```

届时可以进入下一阶段：

```text
Rust data-plane 性能压测与默认启用策略设计
```

下一阶段再考虑：

```text
1. Go 原生路径 vs Rust 路径压测；
2. 高并发 stream RSS / P99 / CPU 对比；
3. 默认启用 Rust data-plane 的 feature flag；
4. 出问题时自动 fallback 到 Go native path；
5. 扩展更多 provider。
```
