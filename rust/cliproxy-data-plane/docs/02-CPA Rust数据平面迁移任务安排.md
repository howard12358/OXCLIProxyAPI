# CPA Rust 数据平面迁移任务安排

## 1. 文档目标

本文档用于把《CPA 管理平面与数据平面设计》进一步落成可执行的任务安排。

近期目标不是一次性替换 CLIProxyAPI，而是在保留现有 Go 管理平面的前提下，引入一个可以逐步接管热路径的 Rust 数据平面，并按阶段迁移最有价值的链路。

## 2. 范围

本文档覆盖设计文档中首批最值得 Rust 化的 4 块：

1. 流量接入与 SSE 转发
2. 上游执行运行时
3. 带会话粘性的路由与鉴权选择
4. 高频协议转换

首期明确不做的内容：

- 管理后台重写
- OAuth 浏览器登录流程重写
- auth 持久化重写
- 插件宿主重写
- TUI 重写
- 第一天就追求所有 provider 的完整对齐

## 3. 执行原则

- Go 继续作为配置、auth 状态和管理写操作的事实来源
- Rust 以 sidecar / 独立服务方式接入
- 按垂直切片交付，不先堆纯框架代码
- 先保证行为兼容，再追求性能优化
- 先补观测能力，再迁关键流量
- 每个阶段都必须可单独回退

## 4. 仓库落点

当前仓库中的 Rust 数据平面目录为：

```text
rust/cliproxy-data-plane/
```

建议逐步演进为：

```text
rust/cliproxy-data-plane/
  docs/
  crates/
    common-types/
    runtime-config-client/
    dataplane-ingress/
    upstream-runtime/
    router-core/
    protocol-translate/
    usage-events/
  src/
    main.rs
```

## 5. 里程碑安排

### 里程碑 0：基础骨架与契约

目标：
先把 Go 和 Rust 之间的安全边界立住，在不迁业务流量的前提下完成基础设施准备。

任务：

- 定义 Rust 消费的 runtime snapshot 结构
- 定义 snapshot 版本字段：`version`、`generated_at`、`source_instance_id`
- 定义 Go 对 Rust 的 snapshot 拉取接口形式
- 定义 Rust 的健康状态：`starting`、`ready`、`degraded`、`failed`
- 定义 ingress、routing、upstream、usage 的首批指标面
- 根据需要把当前 Rust 项目调整为 workspace 结构

交付物：

- runtime snapshot 契约文档
- Rust crate 骨架
- sidecar 启动流程说明
- 健康检查和就绪行为说明

验收标准：

- Rust 服务可以独立启动
- Rust 可以加载静态 snapshot 文件或 mock snapshot
- Rust 侧对 snapshot 的应用是原子性的
- 现有 Go 请求链路暂时不需要改动

### 里程碑 1：Snapshot Client 与运行时状态

目标：
让 Rust 可以从 Go 拉取、校验并应用运行时配置。

任务：

- 实现 `runtime-config-client`
- 实现 snapshot 轮询和版本比较
- 实现 snapshot 结构校验和非法快照拒绝
- 实现运行时状态原子切换
- 当已经成功加载过快照后，后续刷新失败时进入 degraded 而不是直接不可用
- 当从未成功加载过有效快照时，保持 fail closed

交付物：

- runtime config client crate
- snapshot 校验逻辑
- 运行时状态管理组件
- 刷新成功/失败日志和指标

依赖：

- 里程碑 0 完成
- Go 侧 snapshot 接口或等价 fixture 已就绪

验收标准：

- Rust 可以重复拉取 snapshot 而无需重启
- 新旧 snapshot 切换时，在途请求仍可继续使用旧快照完成
- 在已有有效 snapshot 的前提下，刷新失败不会直接打断服务

### 里程碑 2：`/v1/responses` 接入垂直切片

目标：
先把最小但价值最高的一条真实请求链路迁到 Rust。

任务：

- 构建 `dataplane-ingress`，先支持 `/v1/responses`
- 实现请求解析和元数据提取
- 实现下游 SSE 写回
- 实现响应头提交前的 bootstrap 缓冲
- 实现 SSE 帧解析和基础归一化
- 保留 usage 和 terminal 事件
- 增加首字节延迟和流会话时长指标

交付物：

- Rust 版 `/v1/responses` 路由
- bootstrap 缓冲逻辑
- SSE writer / flusher
- 基础错误映射逻辑

依赖：

- 里程碑 1 完成
- 从 Go 当前链路捕获必要测试样本

验收标准：

- Rust 可以在 mock upstream 下完整服务 `/v1/responses`
- bootstrap 成功前不会提交下游响应头
- 客户端看到的 SSE 结构与当前 Go 行为兼容
- 下游断连后资源可以正确清理

### 里程碑 3：OpenAI / Codex 上游执行运行时

目标：
给 Rust ingress 提供第一批真实可用的 upstream 执行能力。

任务：

- 实现 `upstream-runtime`
- 增加 OpenAI Responses HTTP upstream adapter
- 增加 Codex upstream adapter
- 定义统一内部流事件模型
- 实现“下游未发字节前”的重试分类逻辑
- 实现 usage 提取
- 实现请求/响应日志及敏感字段脱敏

交付物：

- OpenAI adapter
- Codex adapter
- 统一流事件模型
- 重试分类器
- usage 提取器

依赖：

- 里程碑 2 完成
- 从当前 Go executor 捕获 provider 请求/响应样本

验收标准：

- Rust ingress 能通过真实 upstream adapter 执行 `/v1/responses`
- bootstrap 阶段支持预提交重试
- usage 元数据在往返链路中不丢失
- 日志中不泄漏敏感信息

### 里程碑 4：Router Core v1

目标：
在不迁 OAuth 生命周期主控权的前提下，把 auth 选择和会话粘性规划迁到 Rust。

任务：

- 实现 `router-core`
- 基于 snapshot 中的 auth 和 model 数据构建索引
- 实现 `fill-first`
- 实现仅在当前最高可用 priority 层内进行 `round-robin`
- 实现带 TTL 的 session affinity map
- 实现 cooldown 和 unhealthy 状态跟踪
- 返回 `ExecutionPlan` 而不是直接发请求
- 保留 `pinned auth`、`execution session identity` 等请求级语义

交付物：

- router core crate
- execution plan 类型
- session affinity 存储
- scheduler 指标

依赖：

- 里程碑 1 完成
- 里程碑 3 提供真实执行能力

验收标准：

- 对同一份 snapshot 和同一类请求上下文，Rust 的选择结果与 Go 的预期行为一致
- 会话粘性请求在策略和健康状态允许时保持命中同一 auth
- retry candidates 在执行计划中明确可见

### 里程碑 5：协议转换 IR v1

目标：
把请求/响应翻译逻辑收拢到 Rust 的统一中间表示，不继续延续 Go 中多处分散的结构。

任务：

- 实现 `protocol-translate`
- 定义 request / response / stream event 的 canonical IR
- 实现 OpenAI Responses 请求到 IR 的解析
- 实现 IR 到 Codex upstream payload 的转换
- 实现 upstream stream events 到 OpenAI Responses SSE 的回转
- 尽可能保留 tool call、reasoning、usage 语义

交付物：

- IR 类型定义
- OpenAI Responses parser / emitter
- Codex request adapter
- Codex event adapter

依赖：

- 里程碑 3 完成

验收标准：

- 代表性的 OpenAI Responses 请求可以经由 IR 转成 Codex，再转回兼容输出
- 已覆盖的关键语义在样本中保持不变
- 翻译逻辑不再分散在 ingress、runtime 多处

### 里程碑 6：SSE 修复与行为对齐

目标：
补齐当前 Go 路径中最关键的兼容性细节，减少迁移后的行为偏差。

任务：

- 实现 `response.completed` 修复逻辑
- 处理非终态 chunk 的边界情况
- 对齐 terminal event 行为
- 增加与 Go 输出样本对照的 parity tests
- 增加 malformed stream 的回归测试

交付物：

- SSE repair 组件
- parity fixtures
- partial / malformed frame 回归测试

依赖：

- 里程碑 2 和 里程碑 5 完成

验收标准：

- 已知不完整 SSE 场景可以被正确修补
- Rust 输出在覆盖样本上与 Go 的预期事件序列一致

### 里程碑 7：扩展路由与 Provider

目标：
在核心链路稳定后，再扩展更多外部路由和 provider。

任务：

- 增加 `/v1/chat/completions`
- 增加 `/v1/messages`
- 增加 Claude upstream adapter
- 增加 Gemini upstream adapter
- 扩展 routing index 支持更多 provider/model 组合

交付物：

- 新增 ingress routes
- Claude adapter
- Gemini adapter

依赖：

- 里程碑 2 到 里程碑 6 稳定

验收标准：

- 新路由在显式开关下可用
- 各 provider 行为有对应兼容样本覆盖

### 里程碑 8：Usage 事件与运维集成

目标：
补齐数据平面的事件输出和可运维能力。

任务：

- 实现 `usage-events`
- 定义回传 Go 或 queue 系统的 usage event 契约
- 输出请求数、延迟、流会话时长、路由分布等指标
- 输出 auth 健康信号和 cooldown 建议
- 整理适合 dashboard/alert 的指标命名

交付物：

- usage event producer
- 指标面
- 运维说明或 runbook 草稿

依赖：

- 里程碑 3 提供 usage 提取
- 里程碑 4 提供 routing 状态

验收标准：

- usage 事件输出不会阻塞热路径
- 指标维度足够稳定，能直接支撑 dashboard 和告警

## 6. 可并行工作

在里程碑 0 之后，可以并行推进的内容：

- Go 侧 snapshot 接口实现
- Rust workspace 结构调整
- 从当前 Go 请求路径抓取 fixture
- 指标命名与观测契约整理

更适合串行推进的内容：

- 真实流量 ingress
- upstream runtime 接入
- router 迁移
- 协议兼容性收口

## 7. 测试策略

### 契约测试

- snapshot 结构校验
- snapshot 向前/向后兼容性
- snapshot 原子切换行为

### 单元测试

- router 选择逻辑
- session affinity TTL 行为
- 重试分类逻辑
- SSE framing 和 repair
- 协议转换 IR

### 集成测试

- `/v1/responses` + mock upstream
- OpenAI / Codex 端到端链路
- 下游取消后的资源清理
- usage 提取和事件输出

### 对齐测试

- 用 Rust 输出对照捕获到的 Go fixtures
- 用 Rust 的 auth 选择结果对照 Go 的代表性行为

## 8. 推荐执行顺序

推荐实际推进顺序：

1. 里程碑 0
2. 里程碑 1
3. 里程碑 2
4. 里程碑 3
5. 里程碑 4
6. 里程碑 5
7. 里程碑 6
8. 里程碑 8
9. 里程碑 7

原因：
最先要拿到的是一条真正可跑的 `/v1/responses` 垂直链路。更多路由和 provider 扩展应当放在契约、运行时行为和兼容性已经相对稳定之后。

## 9. 当前仓库下一步最具体的任务

基于当前仓库状态，最适合立刻推进的事情是：

1. 在 `rust/cliproxy-data-plane/crates/` 下搭出 workspace crate 目录
2. 新增 runtime snapshot 契约文档和示例 payload
3. 先实现一个读取本地 JSON snapshot 文件的最小 `runtime-config-client`
4. 实现 `common-types`，先承载 snapshot、execution plan、stream event 类型
5. 增加一个 mock stream 的 `/v1/responses` 路由，验证 ingress 结构

## 10. 第一版可用发布的退出标准

当下面这些条件全部满足时，可以认为 Rust 数据平面已经达到第一版可用状态：

- Rust 可以基于 snapshot 驱动运行时状态
- Rust 可以端到端服务 `/v1/responses`
- Rust 至少支持 OpenAI 或 Codex 其中一条真实 upstream 执行链路
- Rust 可以在已覆盖场景下完成 session-aware 的 auth 选择
- Rust 保留 usage 和 terminal event 的关键语义
- Rust 输出的指标和事件足以支撑与 Go 并行运行时的安全运维
