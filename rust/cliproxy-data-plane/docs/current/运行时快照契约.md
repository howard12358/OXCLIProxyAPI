# CPA Runtime Snapshot 契约

## 1. 文档目标

本文档定义 Go 管理平面提供给 Rust 数据平面的 runtime snapshot 契约。

Rust 不应直接消费 Go 的完整内部配置，而应消费一份已经规整完成、适合热路径直接使用的只读执行视图。

## 2. 契约定位

这份 snapshot 的职责是：

- 向 Rust 提供当前可用的 listener 配置
- 向 Rust 提供启用的路由信息
- 向 Rust 提供 provider、model、auth 的运行时索引基础数据
- 向 Rust 提供 routing 与 session affinity 配置
- 向 Rust 提供上游出站代理等网络执行策略
- 向 Rust 提供 usage queue 和 feature flag 等数据平面执行参数

这份 snapshot 不负责：

- 承载完整的管理后台配置细节
- 承载 OAuth 生命周期控制逻辑
- 承载 auth 的持久化语义
- 暴露 Go 内部实现细节

## 3. 顶层字段

每份 snapshot 至少应包含：

- `version`
- `generated_at`
- `source_instance_id`
- `listeners`
- `routes`
- `routing`
- `providers`
- `model_aliases`
- `models`
- `auth_pool`
- `network`
- `usage_queue`
- `feature_flags`

## 4. 字段说明

### 4.1 版本元信息

- `version`
  - 当前快照版本号
  - 用于 Rust 侧比较新旧快照
- `generated_at`
  - 当前快照生成时间
- `source_instance_id`
  - 快照来源的 Go 实例标识

### 4.2 listeners

首期最小字段：

- `public_http`
  - Rust 数据平面对外监听地址

### 4.3 routes

用于标记当前启用的路由。

首期建议：

- `responses`
- `chat_completions`
- `messages`

### 4.4 routing

用于控制路由和 session affinity 行为。

首期建议字段：

- `strategy`
  - 可选值：`fill-first`、`round-robin`
- `session_affinity`
  - 是否启用会话粘性
- `session_ttl_seconds`
  - 会话粘性的 TTL

### 4.5 providers

用于声明 provider 级别的启用状态。

首期最小结构：

- provider 名称
- `enabled`

### 4.6 model_aliases

用于提供按 provider 维度组织的 alias 表。

例如：

- `codex.codex-latest -> gpt-5-codex`

### 4.7 models

用于提供 provider 到模型列表的映射。

### 4.8 auth_pool

用于向 Rust 暴露可参与路由的 auth 运行时记录。

首期建议字段：

- `id`
- `provider`
- `priority`
- `enabled`
- `supports_models`
- `labels`
- `cooldown_until`

说明：

- `priority` 是层级，不是权重
- `supports_models` 是该 auth 可服务的模型集合
- `cooldown_until` 用于声明该 auth 是否仍在冷却期

### 4.9 usage_queue

用于描述 usage 事件输出配置。

首期建议字段：

- `enabled`
- `backend`

### 4.10 network

用于描述 Rust 数据平面访问外部上游时的默认网络策略。

首期建议字段：

- `upstream_proxy`
  - Go 下发给 Rust 的默认上游代理策略
  - 仅作用于 `Rust -> Provider` 的外部请求
  - 不作用于 `Go <-> Rust` 本地通信

`upstream_proxy` 的语义建议固定为以下 3 类：

1. 空字符串
   - `inherit`
   - 表示不在 snapshot 中显式指定代理
   - Rust 可退回到本地 CLI / env / 运行时默认行为

2. `direct`
   - 强制直连
   - 明确禁止对上游请求使用任何代理

3. 显式代理 URL
   - 例如 `http://127.0.0.1:7897`
   - 例如 `https://proxy.example.com:8443`
   - 例如 `socks5://127.0.0.1:7897`
   - 例如 `socks5h://127.0.0.1:7897`

补充约束：

- `Go -> Rust`
- `Rust -> Go`

这两类本地控制面通信不应使用 `upstream_proxy`。

也就是说：

- `GET /v0/management/runtime-snapshot`
- Go 转发 `POST /v1/responses` 到 Rust
- Go 探活 Rust `/healthz`、`/readyz`

都必须保持 loopback / 内网直连，不应被全局出口代理劫持。

### 4.11 feature_flags

用于做数据平面内部能力开关，例如：

- `enable_sse_repair`
- `enable_responses_route`

## 5. 推荐 JSON 示例

```json
{
  "version": "2026-06-10T00:00:00Z#1",
  "generated_at": "2026-06-10T00:00:00Z",
  "source_instance_id": "go-cpa-main-01",
  "listeners": {
    "public_http": ":8317"
  },
  "routes": {
    "responses": true,
    "chat_completions": false,
    "messages": false
  },
  "routing": {
    "strategy": "fill-first",
    "session_affinity": true,
    "session_ttl_seconds": 3600
  },
  "providers": {
    "openai": {
      "enabled": true
    },
    "codex": {
      "enabled": true
    }
  },
  "model_aliases": {
    "codex": {
      "codex-latest": "gpt-5-codex"
    }
  },
  "models": {
    "codex": [
      "gpt-5-codex",
      "gpt-5-codex-mini"
    ]
  },
  "auth_pool": [
    {
      "id": "auth_codex_01",
      "provider": "codex",
      "priority": 100,
      "enabled": true,
      "supports_models": [
        "gpt-5-codex",
        "gpt-5-codex-mini"
      ],
      "labels": [
        "paid"
      ],
      "cooldown_until": null
    }
  ],
  "network": {
    "upstream_proxy": "socks5://127.0.0.1:7897"
  },
  "usage_queue": {
    "enabled": true,
    "backend": "redis"
  },
  "feature_flags": {
    "enable_sse_repair": true
  }
}
```

## 6. Rust 侧约束

Rust 侧应遵循这些规则：

- snapshot 必须整体校验后再应用
- snapshot 应原子切换
- 在途请求继续使用旧 snapshot 完成
- 新请求使用最新成功应用的 snapshot
- 若从未成功加载有效 snapshot，应保持 fail closed
- 若已经成功加载过 snapshot，后续刷新失败时应进入 degraded 而不是立即不可用

Rust 对 `network.upstream_proxy` 还应遵循以下规则：

- 仅将其用于访问外部 provider 的请求
- 支持的代理协议至少包括：
  - `http`
  - `https`
  - `socks5`
  - `socks5h`
- `direct` 表示显式关闭代理
- 空值表示 `inherit`
- `inherit` 不是“继承 Go 代理对象”，而是“允许 Rust 退回到自身运行时默认配置来源”

首期建议的 Rust 侧优先级：

1. Rust 显式 CLI 参数
2. Rust 显式环境变量
3. snapshot `network.upstream_proxy`
4. 空值 / 默认行为

## 7. 里程碑 0 与里程碑 1 的边界

里程碑 0 完成的内容：

- 契约字段定义
- 示例 payload
- Rust 基础类型结构

里程碑 1 再完成的内容：

- 本地文件和 HTTP 拉取实现
- schema 校验
- 版本比较
- 原子切换和 degraded 行为
