# CPA 与 Data Plane 通信矩阵

## 1. 文档目标

本文档用于明确当前设计下 Go 管理平面（CPA）与 Rust 数据平面（Data Plane）之间的通信关系，帮助后续实现时区分：

- 哪些通信属于控制面
- 哪些通信属于热路径
- 哪些通信已经实现
- 哪些通信仍停留在设计阶段

## 2. 总体原则

当前设计下，CPA 与数据平面的通信遵循以下原则：

- Go 负责配置、auth 状态、管理写操作和后台维护任务
- Rust 负责请求接入、流式转发、上游执行、协议转换和热路径状态
- 请求主链路尽量不再经过 Go
- Go 与 Rust 的交互应尽量放在控制面通信和运行反馈层，而不是把 Go 放回数据热路径中

一句话概括：

`Go 负责告诉 Rust 该怎么跑，Rust 按快照独立跑流量，再把运行结果和健康信号回传。`

## 3. 通信矩阵

| 方向 | 发起方 | 接收方 | 内容 | 建议协议 | 是否热路径 | 当前状态 |
|---|---|---|---|---|---|---|
| 配置下发 | Rust 拉取 | Go CPA | `runtime snapshot` 全量快照 | 本地 loopback HTTP | 否 | 设计已定，Go 未实现 |
| 快照刷新 | Rust 定时拉取 | Go CPA | 最新 `version` 对应的运行时配置 | 本地 loopback HTTP | 否 | Rust 已支持，Go 未实现 |
| 首次启动校验 | Rust | Go CPA | 首次 snapshot 获取 | 本地 loopback HTTP | 否 | Rust 已支持，Go 未实现 |
| 流量接入 | Client | Rust Data Plane | `/v1/responses`、后续 `/v1/chat/completions` 等请求 | HTTP / SSE | 是 | Rust 已有 mock ingress |
| 上游执行 | Rust Data Plane | Provider | OpenAI / Codex / Claude / Gemini 请求 | HTTP / WebSocket / SSE | 是 | 设计中，未实现真实 runtime |
| usage 输出 | Rust Data Plane | Go CPA 或队列系统 | usage 事件 | queue / HTTP / 本地接口 | 否 | 仅设计，未实现 |
| 健康信号回传 | Rust Data Plane | Go CPA | `ready / degraded / failed`、错误摘要 | 管理接口 / 指标抓取 | 否 | Rust 本地有状态，Go 未接入 |
| auth 健康回传 | Rust Data Plane | Go CPA | auth unhealthy、cooldown 建议 | 管理接口 / 事件流 | 否 | 仅设计 |
| 指标采集 | 监控系统或 Go | Rust Data Plane | 请求数、首字节延迟、流时长等指标 | HTTP metrics endpoint | 否 | 仅设计 |
| 管理写操作 | Operator / Go 管理面 | Go CPA | 配置修改、auth 管理、OAuth 生命周期控制 | Go 内部管理 API | 否 | 已在 Go 侧存在 |
| 热路径绕过 | Rust Data Plane | Client / Provider | 请求与响应主链路 | 直接连接 | 是 | 目标架构，部分已起步 |

## 4. 当前最关键的两条通信链

### 4.1 Go 到 Rust

当前最重要的控制面通信是：

- Go 提供 `runtime snapshot`
- Rust 周期性拉取并原子应用

这条链的特点是：

- 不在业务热路径上
- Go 是事实来源
- Rust 是快照消费者
- 配置更新通过版本号驱动，而不是推送式强耦合控制

### 4.2 Rust 到 Go

当前最重要的反馈面通信是：

- usage 事件
- 健康状态
- auth 健康信号
- cooldown 建议
- 严重错误信号

这条链当前还没有完全定死协议，但已经明确这类回传不应把 Go 重新拉回请求主链路。

## 5. 热路径与非热路径边界

### 5.1 热路径

真正属于热路径的通信是：

- `Client -> Rust Data Plane`
- `Rust Data Plane -> Provider`
- `Provider -> Rust Data Plane`
- `Rust Data Plane -> Client`

这些通信必须由 Rust 独立承担，Go 不应重新插入中间转发。

### 5.2 非热路径

不属于热路径但必须存在的通信是：

- Rust 拉取 Go snapshot
- Rust 回传 usage 和健康信号
- Go 管理面写入配置和 auth 状态

这些通信更关注正确性、可恢复性和可观测性，而不是单请求延迟。

## 6. 当前实现状态

### 6.1 Rust 已实现

- 本地文件 snapshot 拉取
- HTTP snapshot 拉取能力
- snapshot 基础校验
- snapshot 版本比较
- 运行时状态切换：`ready / degraded / failed`
- `/healthz`
- `/readyz`
- `/v1/responses` mock ingress

### 6.2 Rust 未实现

- 真实 upstream runtime
- usage 事件输出
- auth 健康信号回传
- cooldown 建议回传
- metrics endpoint

### 6.3 Go 未实现

- 正式的 runtime snapshot 导出接口
- 消费 Rust usage / 健康 / auth 信号的接口
- 真实流量切到 Rust 数据平面的入口

## 7. 建议的最小落地顺序

从通信矩阵看，最先应当打通的不是所有通信，而是最小闭环：

1. Go 导出 runtime snapshot
2. Rust 从 Go 拉真实 snapshot
3. Rust 用真实 snapshot 驱动 `/v1/responses`
4. Rust 向外暴露可供观测的健康和指标
5. 再逐步增加 usage / auth 健康 / cooldown 回传

## 8. 结论

当前设计里的 CPA 与数据平面通信，本质上不是“两个服务互相代理请求”，而是：

- Go 负责控制信息和状态来源
- Rust 负责数据热路径执行
- 两边通过 snapshot 和运行反馈进行协作

因此，最重要的首条正式通信链不是 usage，也不是 metrics，而是：

`Go -> runtime snapshot -> Rust`

这条链一旦打通，Rust 数据平面才算真正开始与 CPA 发生实际联动。
