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

### 4.10 feature_flags

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
