# Rust 数据平面文档索引

本目录按“当前事实 / 路线图 / 设计方案 / 历史过程”分层，避免把当前实现说明、长期设计稿和阶段性迁移记录混在一起。

## 目录约定

- `current/`
  - 当前事实文档
  - 适合在修改实现前优先阅读
- `roadmap/`
  - 当前迁移任务和里程碑状态
  - 适合判断下一步优先级
- `design/`
  - 仍有参考价值的设计方案
  - 描述为什么这样设计、未来准备怎么演进
- `history/`
  - 阶段性迁移计划、早期状态说明、通信矩阵等历史材料
  - 主要用于回溯背景，不应直接当作当前实现事实来源

## current

- [当前架构说明](./current/当前架构说明.md)
- [运行时快照契约](./current/运行时快照契约.md)
- [v1 接口矩阵说明](./current/v1接口矩阵说明.md)
- [CPA 用量流程与 RS 差距](./current/CPA用量流程与RS差距.md)

## roadmap

- [Rust 数据平面迁移任务安排](./roadmap/Rust数据平面迁移任务安排.md)

## design

- [管理平面与数据平面拆分设计](./design/管理平面与数据平面拆分设计.md)
- [Go 管理 Rust 数据平面产品化设计](./design/Go管理Rust数据平面产品化设计.md)

## history

- [Runtime Snapshot 导出器设计与实施方案](./history/Runtime%20Snapshot导出器设计与实施方案.md)
- [Rust Sidecar 启动与健康状态说明（里程碑 0 阶段）](./history/Rust%20Sidecar启动与健康状态说明（里程碑0阶段）.md)
- [CPA 与 Data Plane 通信矩阵（阶段记录）](./history/CPA与Data%20Plane通信矩阵（阶段记录）.md)

## 使用建议

- 想看“现在系统是什么样”：优先读 `current/`
- 想看“接下来做什么”：读 `roadmap/`
- 想看“为什么这么设计”：读 `design/`
- 想看“当时如何迁移、哪些内容已经过时”：读 `history/`
