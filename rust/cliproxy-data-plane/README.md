# cliproxy-data-plane

面向 CLIProxyAPI 数据平面职责的 Rust sidecar 骨架。

## 当前范围

- 基于 `tokio` 的二进制服务
- 基于 `axum` 的 HTTP 服务
- 基础健康检查接口：`/healthz`、`/readyz`
- 基于命令行和环境变量的监听地址与日志级别配置
- 里程碑 0 所需的 workspace 基础结构
- runtime snapshot 契约基础类型
- 本地文件和 HTTP snapshot 拉取
- snapshot 校验、版本比较和运行时状态切换

## 当前目录结构

```text
rust/cliproxy-data-plane/
  docs/
  crates/
    common-types/
    runtime-config-client/
  src/
    main.rs
```

## 运行方式

```bash
cargo run -- --bind-addr 127.0.0.1:4100 --snapshot-file examples/runtime-snapshot.example.json
```

或使用 `Makefile`：

```bash
make run
make run BIND_ADDR=127.0.0.1:4200 LOG_LEVEL=debug
```

环境变量：

- `CLIPROXY_BIND`
- `CLIPROXY_LOG`
- `CLIPROXY_SNAPSHOT_FILE`
- `CLIPROXY_SNAPSHOT_URL`
- `CLIPROXY_SNAPSHOT_POLL_SECONDS`

常用命令：

- `make help`
- `make fmt`
- `make check`
- `make test`
- `make build`

## 当前里程碑状态

当前已经完成里程碑 0 和里程碑 1 的基础落地：

- 建立 Rust workspace 结构
- 建立 `common-types` crate
- 建立 `runtime-config-client` crate
- 定义 `runtime snapshot` 基础结构
- 定义服务健康状态枚举
- 补充 snapshot 契约和 sidecar 启动说明文档
- 支持本地文件和 HTTP snapshot 拉取
- 支持 snapshot 校验和版本比较
- 支持运行时状态的 `ready / degraded / failed` 切换

## 下一步

- 开始实现 `/v1/responses` ingress 垂直切片
