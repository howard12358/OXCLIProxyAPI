# CPA Rust Sidecar 启动与健康状态说明

## 1. 文档目标

本文档说明 Rust 数据平面 sidecar 在里程碑 0 阶段的启动职责、健康状态含义，以及 `/healthz` 和 `/readyz` 的预期语义。

## 2. 当前启动职责

在里程碑 0 阶段，Rust sidecar 的职责仍然非常轻：

- 解析命令行参数和环境变量
- 初始化日志
- 绑定监听地址
- 暴露健康检查接口
- 为后续 runtime snapshot 和 ingress 逻辑预留结构

当前不负责：

- 实际拉取 Go snapshot
- 实际转发业务流量
- 实际执行 upstream 请求

## 3. 健康状态定义

Rust sidecar 统一使用以下健康状态：

- `starting`
  - 进程已启动，但尚未具备服务能力
- `ready`
  - 当前进程可以正常对外提供已启用的功能
- `degraded`
  - 当前进程仍可提供部分能力，但存在非致命问题
- `failed`
  - 当前进程不应继续承担流量

## 4. 当前阶段的状态语义

在里程碑 0 阶段：

- 进程启动并绑定监听成功后，状态可视为 `ready`
- 由于尚未接入 snapshot 拉取逻辑，因此不会真正进入 `degraded`
- 若未来接入 snapshot 拉取：
  - 已成功加载过快照，后续刷新失败应进入 `degraded`
  - 从未成功加载过快照，则应保持 `starting` 或进入 `failed`

## 5. 健康检查接口语义

### 5.1 `/healthz`

用途：

- 表示进程是否存活
- 返回当前服务状态枚举

当前阶段预期：

- 只要服务进程还在并能处理请求，就返回当前状态

### 5.2 `/readyz`

用途：

- 表示进程是否已就绪，可以承担当前阶段定义的职责

当前阶段预期：

- 当 HTTP 服务已成功启动并进入 `ready` 状态时，返回 `ready=true`

未来阶段预期：

- 只有在关键依赖已满足时才返回 `ready=true`
- 例如：snapshot 已成功加载、路由能力已初始化、关键组件已可用

## 6. 当前返回结构

当前代码中的健康接口返回结构可概括为：

- `/healthz`
  - `status`
  - `service`
  - `version`

- `/readyz`
  - `ready`
  - `status`
  - `runtime`

其中 `runtime` 当前至少包含：

- `service`
- `version`
- `bind_addr`
- `state`

## 7. 后续扩展建议

里程碑 1 之后，建议逐步扩展这些信息：

- snapshot 当前版本
- 最近一次快照刷新时间
- 最近一次快照刷新是否成功
- 当前已启用路由列表
- 当前状态进入原因

这样可以让 Go 管理平面或运维工具更准确地判断 Rust sidecar 是否可以接流量。
