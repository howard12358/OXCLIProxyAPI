# 0002-rust-docs-structure.md

## Status

Accepted

## Context

`rust/cliproxy-data-plane/docs/` 目录内同时存在三类内容：

- 当前实现事实文档
- 面向未来演进的设计方案
- 阶段性迁移计划和历史状态记录

原目录仅靠 `01` 到 `09` 的编号排列，能反映大致时间顺序，但不能反映文档用途。随着 Rust 数据平面文档增多，读者很难快速判断：

- 哪些文档可以当作当前实现事实来源
- 哪些只是设计稿
- 哪些已经偏历史背景

## Decision

将 `rust/cliproxy-data-plane/docs/` 重组为按用途分类的结构：

- `current/`
  - 存放当前事实文档
- `roadmap/`
  - 存放迁移任务、里程碑状态和后续优先级
- `design/`
  - 存放仍有参考价值的设计方案
- `history/`
  - 存放早期状态说明、已落地设计记录和历史材料

同时新增 `rust/cliproxy-data-plane/docs/README.md` 作为目录索引，并使用描述性中文文件名代替单纯的数字前缀。

2026-07-03 修订：

- 保留 `current/`、`roadmap/`、`design/`、`history/` 的用途分类。
- Rust 数据平面文档文件名统一使用中文命名，保持与现有文档语言一致：
  - `current/当前架构说明.md`
  - `current/运行时快照契约.md`
  - `current/v1接口矩阵说明.md`
  - `current/用量队列契约与差距.md`
  - `roadmap/Rust数据平面迁移路线图.md`
  - `design/Go管理Rust数据平面产品化设计.md`
- 早期《管理平面与数据平面拆分设计》移动到 `history/`，作为背景材料保留，不再作为当前实现事实来源。
- `docs/README.md` 明确事实来源优先级：`.ai-harness/shared/` 优先于 `docs/current/`，`history/` 不覆盖当前事实。

同日二次修订：

- 删除已被当前文档覆盖的历史阶段记录：
  - `history/运行时快照导出器设计与实施方案.md`
  - `history/Rust边车启动与健康状态说明（里程碑0阶段）.md`
  - `history/CPA与数据平面通信矩阵（阶段记录）.md`
- 将通信矩阵中仍然有效的 Go/Rust 热路径与非热路径边界合并进 `current/当前架构说明.md`。
- `history/` 当前只保留早期拆分设计作为背景材料。

## Consequences

优点：

- 当前事实与历史材料分离，阅读路径更清晰
- 迁移计划从历史材料中拆出，避免“仍在推进的任务”被误读为纯归档
- 后续新增文档时可以按用途归类，而不是继续堆编号
- 降低把旧设计稿误当成当前实现事实来源的风险

代价与风险：

- 旧路径失效，需要同步更新仓库内引用
- 外部书签或历史讨论里提到的旧文件名需要人工适配

## Alternatives Considered

- 保持现状，仅继续增加编号文档
  - rejected，因为问题核心不是数量，而是类型混杂
- 只重命名文件，不增加子目录
  - rejected，因为无法从目录层面区分 current / design / history
