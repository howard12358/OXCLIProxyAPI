# 0003-rust-responses-module-split.md

## Status

Accepted

## Context

`rust/cliproxy-data-plane/src/responses.rs` 之前同时承载了：

- `/v1/responses` HTTP handler 入口
- execution plan 生成
- 上游请求归一化与重试
- SSE 分帧与完成事件修复
- mock fallback
- 单元测试

随着 Rust 数据平面补上真实 upstream 执行和 SSE framer，这个文件已经变成单点高复杂度文件。继续在单文件内演进，会带来几个问题：

- handler、上游执行、SSE 修复和 mock 逻辑边界不清
- 测试只能围绕一个大模块组织，难以按责任收束
- 后续继续补 CPA 行为时，更容易引入跨职责改动

这次变更不改变对外 API，只调整 Rust `/v1/responses` 内部模块边界。

## Decision

把 `rust/cliproxy-data-plane/src/responses.rs` 作为父模块保留，并把子职责拆到目录模块：

- `src/responses.rs`
  - 共享请求/错误类型
  - 共享响应头和通用辅助函数
  - `responses` 级别单元测试
- `src/responses/handler.rs`
  - `handle_responses`
  - request 前置校验
  - execution plan 构建
  - mock / real upstream 分流
- `src/responses/upstream.rs`
  - 真实 upstream 执行
  - Codex 请求归一化
  - auth retry chain
  - 流聚合与错误日志
- `src/responses/sse.rs`
  - SSE frame 重组
  - `response.completed.output` 修复
  - SSE payload 提取与 frame 归一化
- `src/responses/mock.rs`
  - mock 非流式 / 流式响应
  - mock SSE 事件拼装

## Consequences

优点：

- `/v1/responses` 主链路按职责分层，更容易继续对齐 CPA 行为
- SSE 修复逻辑与上游执行逻辑解耦，后续扩展风险更低
- 测试可以继续留在 `responses` 模块级别，同时实现代码边界更清晰

代价与风险：

- 内部源文件路径发生变化，文档和代码定位需要同步更新
- 父模块入口和子模块目录并存，需要保持命名清晰

## Alternatives Considered

- 保持单文件，仅增加注释分区
  - rejected，因为复杂度问题主要来自职责耦合，不是缺少注释
- 一次性拆成更细的 provider-specific 或 retry-specific 模块
  - rejected，因为当前范围仍是 `/v1/responses` MVP，先做最小可维护拆分更稳妥
