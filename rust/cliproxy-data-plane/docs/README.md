# Rust 数据平面文档索引

本目录按“当前事实 / 路线图 / 设计方案 / 历史过程”分层，避免把当前实现说明、长期设计稿和阶段性迁移记录混在一起。

事实来源优先级：

1. 仓库级事实：`.ai-harness/shared/`
2. Rust 数据面当前事实：`docs/current/`
3. 后续执行方向：`docs/roadmap/`
4. 设计参考：`docs/design/`
5. 历史背景：`docs/history/`

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
  - 早期设计背景和迁移过程材料
  - 主要用于回溯背景，不应直接当作当前实现事实来源

## current

- [当前架构说明](./current/当前架构说明.md)
- [运行时快照契约](./current/运行时快照契约.md)
- [v1 接口矩阵说明](./current/v1接口矩阵说明.md)
- [Responses 到 Codex 兼容矩阵](./current/Responses到Codex兼容矩阵.md)
- [用量队列契约与差距](./current/用量队列契约与差距.md)

## roadmap

- [Rust 数据平面迁移路线图](./roadmap/Rust数据平面迁移路线图.md)

## design

- [Go 管理 Rust 数据平面产品化设计](./design/Go管理Rust数据平面产品化设计.md)

## history

- [管理平面与数据平面拆分设计（早期设计）](./history/管理平面与数据平面拆分设计（早期设计）.md)

## 使用建议

- 想看“现在系统是什么样”：优先读 `current/`
- 想看“接下来做什么”：读 `roadmap/`
- 想看“为什么这么设计”：读 `design/`
- 想看“当时如何迁移、哪些内容已经过时”：读 `history/`
- `history/` 中的“当前状态”只代表当时阶段，不能覆盖 `current/` 和 `.ai-harness/shared/`。
