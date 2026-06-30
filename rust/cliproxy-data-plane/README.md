# cliproxy-data-plane

面向 CLIProxyAPI 数据平面职责的 Rust sidecar 骨架。

## 当前范围

- 基于 `tokio` 的二进制服务
- 基于 `axum` 的 HTTP 服务
- 基础健康检查与运行时观测接口：`/healthz`、`/readyz`
- 基于命令行和环境变量的监听地址与日志级别配置
- 里程碑 0 所需的 workspace 基础结构
- runtime snapshot 契约基础类型
- 本地文件和 HTTP snapshot 拉取
- snapshot 校验、版本比较和运行时状态切换
- `/v1/responses` handler / upstream / SSE 分层实现
- OpenAI / Codex 上游执行运行时 v1
- `/v1/responses` 的最小 CPA usage queue 闭环

## 当前目录结构

```text
rust/cliproxy-data-plane/
  docs/
    README.md
    current/
    design/
    history/
  crates/
    common-types/
    runtime-config-client/
    usage-events/
  src/
    main.rs
    responses.rs
    responses/
      handler.rs
      upstream.rs
      sse.rs
      mock.rs
    telemetry.rs
```

文档入口：

- `docs/README.md`
- `docs/current/` 存放当前事实文档
- `docs/design/` 存放设计方案
- `docs/history/` 存放阶段性迁移和历史材料

说明：

- `src/responses/mock.rs` 仍作为历史文件保留在仓库里，但当前不参与活跃编译图，也不再承担生产路径 fallback。

## 运行方式

```bash
cargo run -- \
  --bind-addr 127.0.0.1:4100 \
  --snapshot-url http://127.0.0.1:8317/v0/management/runtime-snapshot \
  --snapshot-bearer-token test-management-key
```

或使用 `Makefile`：

```bash
make run
make run BIND_ADDR=127.0.0.1:4200 LOG_LEVEL=debug
```

默认开发方式应连接 Go 管理面导出的真实 `runtime snapshot`，而不是依赖仓库内静态示例文件。

如果只是在脱离 Go 的情况下做临时本地实验，代码仍然支持显式传入 `--snapshot-file`，但这不再作为默认示例路径。

环境变量：

- `CLIPROXY_BIND`
- `CLIPROXY_LOG`
- `CLIPROXY_SNAPSHOT_FILE`
- `CLIPROXY_SNAPSHOT_URL`
- `CLIPROXY_SNAPSHOT_POLL_SECONDS`
- `CLIPROXY_UPSTREAM_PROXY`
- `CLIPROXY_UPSTREAM_HTTP_PROXY`
- `CLIPROXY_UPSTREAM_HTTPS_PROXY`
- `CLIPROXY_OPENAI_BASE_URL`
- `CLIPROXY_OPENAI_API_KEY`
- `CLIPROXY_CODEX_BASE_URL`
- `CLIPROXY_CODEX_TOKEN`
- `CLIPROXY_CODEX_USER_AGENT`
- `CLIPROXY_CODEX_OPENAI_BETA`

常用命令：

- `make help`
- `make fmt`
- `make check`
- `make test`
- `make build`

## 当前可测试接口

- `GET /healthz`
- `GET /readyz`
- `POST /v1/responses`

示例：

```bash
curl -N http://127.0.0.1:4210/v1/responses \
  -H 'content-type: application/json' \
  -d '{
    "model": "gpt-5-codex",
    "stream": true,
    "input": "hello from rust dataplane"
  }'
```

如果要走真实 OpenAI upstream：

```bash
CLIPROXY_OPENAI_API_KEY=your_openai_key \
make run BIND_ADDR=127.0.0.1:4210
```

如果要走真实 Codex upstream：

```bash
CLIPROXY_CODEX_TOKEN=your_codex_token \
make run BIND_ADDR=127.0.0.1:4210
```

如果需要统一指定 Rust 上游代理，优先使用：

```bash
CLIPROXY_UPSTREAM_PROXY=socks5h://127.0.0.1:7897 \
make run BIND_ADDR=127.0.0.1:4210
```

兼容模式下也仍支持：

- `CLIPROXY_UPSTREAM_HTTP_PROXY`
- `CLIPROXY_UPSTREAM_HTTPS_PROXY`

## 当前里程碑状态

当前已经完成里程碑 0 到里程碑 3 的基础落地：

- 建立 Rust workspace 结构
- 建立 `common-types` crate
- 建立 `runtime-config-client` crate
- 定义 `runtime snapshot` 基础结构
- 定义服务健康状态枚举
- 补充 snapshot 契约和 sidecar 启动说明文档
- 支持本地文件和 HTTP snapshot 拉取
- 支持 snapshot 校验和版本比较
- 支持运行时状态的 `ready / degraded / failed` 切换
- 提供 `/v1/responses` ingress
- 仅在可构造真实 upstream 时执行 `/v1/responses`
- 真实 upstream 不可用时直接返回错误，不再走本地 mock fallback
- 支持 OpenAI Responses HTTP upstream
- 支持 Codex bearer-token HTTP upstream
- 支持预提交 bootstrap、流式转发和非流式回包

## 下一步

- 补齐 CPA redis usage 协议侧的对外消费链路
