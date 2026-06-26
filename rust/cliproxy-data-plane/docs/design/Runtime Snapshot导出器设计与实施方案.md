# CPA Runtime Snapshot导出器设计与实施方案

## 1. 文档目标

本文档定义 CPA Go 侧如何新增一个面向 Rust Data Plane 的 `runtime snapshot exporter`。

目标不是改写 CPA 现有请求热路径，而是在现有管理面和运行时 auth 状态之上，新增一层只读导出视图，供 Rust 通过 HTTP 拉取并独立执行 `/v1/responses` 流量。

## 2. 设计原则

- 尽量加新层，不改旧链路
- 不改现有 `/v1/responses`、conductor、executor 主路径
- 复用现有 `config`、`authManager`、`watcher/synthesizer` 结果
- 先只覆盖当前最小闭环：`Codex + /v1/responses + Codex OAuth`

这意味着第一阶段主要新增：

- snapshot 导出 DTO
- snapshot 构建器
- management handler
- management route

## 3. 导出接口

新增管理接口：

- `GET /v0/management/runtime-snapshot`

接口定位：

- 归属于现有 management API
- 复用现有 management 鉴权和可用性控制
- 返回 Rust 数据平面可直接消费的 `RuntimeSnapshot` JSON

第一阶段不新增新的独立服务，也不新增新的持久化文件。

## 4. 数据来源

snapshot 构建只依赖两类现有数据：

1. CPA 配置
   - `*config.Config`
   - 提供 routing、OAuth model alias、Codex header defaults 等静态配置

2. CPA 运行时 auth 状态
   - `*sdk/cliproxy/auth.Manager`
   - 通过 `authManager.List()` 获取当前运行时 auth 快照
   - 这些 auth 已经经过现有 synthesizer、config 合并和运行时状态维护

因此 exporter 不应重新读取 auth 文件，也不应重新实现 auth 合成逻辑。

## 5. 首期导出范围

### 5.1 路由范围

只导出：

- `routes.responses = true`

暂不导出：

- `/v1/chat/completions`
- `/v1/messages`

对应 snapshot 中：

- `chat_completions = false`
- `messages = false`

### 5.2 provider 范围

只处理：

- `provider == codex`
- `auth_kind == oauth`

不处理：

- Codex API key
- Claude
- Gemini
- XAI
- Antigravity
- Kimi

## 6. 运行时映射规则

### 6.1 顶层字段

- `version`
  - 基于导出内容生成稳定哈希
  - 应随 auth pool、模型列表、alias、routing 变化而变化
- `generated_at`
  - 当前 UTC RFC3339 时间
- `source_instance_id`
  - 优先使用本机 hostname
  - 获取失败时回退为 `local-cpa`

### 6.2 routing

从 `cfg.Routing` 映射：

- `strategy`
  - `fill-first` / `fillfirst` / `ff` 统一导出为 `fill-first`
  - 其他情况导出为 `round-robin`
- `session_affinity`
  - 直接使用 `cfg.Routing.SessionAffinity`
- `session_ttl_seconds`
  - 解析 `cfg.Routing.SessionAffinityTTL`
  - 默认值为 `3600`
  - 非法值回退到 `3600`

### 6.3 model_aliases

从 `cfg.OAuthModelAlias["codex"]` 导出：

- key：`Alias`
- value：`Name`

仅导出满足以下条件的 alias：

- `Alias` 非空
- `Name` 非空
- `Alias != Name`
- `Name` 至少被一个可导出 auth 支持

### 6.4 models

`models.codex` 取所有可导出 Codex OAuth auth 的 `supports_models` 并集。

每个 auth 的 `supports_models` 来源于：

- 其 `plan_type` 对应的 Codex 静态模型集合
- 应用 `excluded_models`

说明：

- 这里保留真实模型名，不写 alias
- alias 通过 `model_aliases.codex` 提供

### 6.5 auth_pool

导出条件：

- `provider == codex`
- `auth_kind == oauth`
- 非 disabled
- 有 `access_token`

字段映射：

- `id <- Auth.ID`
- `provider <- Auth.Provider`
- `auth_kind <- Auth.Attributes["auth_kind"]`
- `priority <- Auth.Attributes["priority"]`
- `enabled <- true`
- `supports_models <- 该 auth 对应模型集合`
- `labels <- plan_type`
- `cooldown_until <- NextRetryAfter / Quota.NextRecoverAt`

### 6.6 execution.codex

从运行时 auth 中导出：

- `access_token <- Metadata["access_token"]`
- `account_id <- Metadata["account_id"]`
- `base_url <- Attributes["base_url"]`，否则 `Metadata["base_url"]`，再否则默认 Codex base URL
- `user_agent <- cfg.CodexHeaderDefaults.UserAgent`，为空则回退 Codex executor 当前默认 UA
- `openai_beta <- "responses=v1"`

## 7. 实现分层

建议新增 `internal/dataplane/snapshot` 包，职责如下：

- 定义 Go 版 snapshot DTO
- 提供 `BuildRuntimeSnapshot(cfg, authManager, now)` 构建函数
- 负责 provider/auth 过滤
- 负责 routing 规范化
- 负责模型集合计算
- 负责版本哈希生成

management handler 只负责：

- 调用构建器
- 处理错误
- 返回 JSON

这样可以把业务映射逻辑从 management handler 中剥离出来，降低后续 merge 冲突面。

## 8. 路由接线原则

只在 management route 注册处新增一条路由：

- `GET /v0/management/runtime-snapshot`

不修改：

- OpenAI / Codex 对外请求路由
- executor 注册逻辑
- 现有 auth 选择逻辑

## 9. 测试要求

至少覆盖：

1. snapshot builder 单测
   - 正常导出 Codex OAuth auth
   - disabled auth 不导出
   - 无 token auth 不导出
   - alias 能正确导出
   - cooldown 能正确导出
   - version 对内容变化敏感

2. management handler / route 测试
   - management 未启用时接口不可访问
   - management 启用后返回 200
   - 返回 JSON 含 `routes.responses`、`providers.codex`、`auth_pool`

3. 联调验证
   - Rust 指向 CPA snapshot endpoint
   - Rust 能成功加载 snapshot
   - Rust `/v1/responses` 能走到选中的 Codex OAuth auth

## 10. 实施顺序

建议按下面顺序落地：

1. 写 exporter DTO 与 builder
2. 写 builder 单测
3. 新增 management handler
4. 注册 route
5. 写 route 测试
6. 用 Rust `--snapshot-url` 做本地联调

## 11. 当前阶段不做的事情

当前设计明确不包含：

- usage event 回传
- auth unhealthy / cooldown 建议回传
- metrics endpoint
- Rust 回推健康状态到 CPA
- 非 Codex provider 的 snapshot 导出
- 将 CPA 热路径切换到 Rust 数据平面

第一阶段只解决：

`CPA 能把 Codex OAuth 运行时状态稳定导出给 Rust，Rust 能据此独立完成 /v1/responses 上游执行。`
