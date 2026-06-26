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

将 `rust/cliproxy-data-plane/docs/` 重组为三层结构：

- `current/`
  - 存放当前事实文档
- `design/`
  - 存放仍有参考价值的设计方案
- `history/`
  - 存放阶段性迁移计划、早期状态说明和历史材料

同时新增 `rust/cliproxy-data-plane/docs/README.md` 作为目录索引，并使用描述性中文文件名代替单纯的数字前缀。

## Consequences

优点：

- 当前事实与历史材料分离，阅读路径更清晰
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
