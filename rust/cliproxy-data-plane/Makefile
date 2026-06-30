APP_NAME := cliproxy-data-plane
BIND_ADDR ?= 127.0.0.1:4100
LOG_LEVEL ?= info
SNAPSHOT_URL ?= http://127.0.0.1:8317/v0/management/runtime-snapshot
SNAPSHOT_BEARER_TOKEN ?= test-management-key

.PHONY: help run fmt check test build clean

help:
	@printf "%s\n" \
	"可用目标：" \
	"  make run    启动 Rust 数据平面服务" \
	"  make fmt    格式化所有 Rust 代码" \
	"  make check  执行 cargo check" \
	"  make test   执行 cargo test" \
	"  make build  构建 debug 二进制" \
	"  make clean  清理 target 目录" \
	"" \
	"可选变量：" \
	"  BIND_ADDR=<host:port>  默认 $(BIND_ADDR)" \
	"  LOG_LEVEL=<level>      默认 $(LOG_LEVEL)" \
	"  SNAPSHOT_URL=<url>     默认 $(SNAPSHOT_URL)" \
	"  SNAPSHOT_BEARER_TOKEN=<token>  默认 $(SNAPSHOT_BEARER_TOKEN)"

run:
	CLIPROXY_BIND=$(BIND_ADDR) \
	CLIPROXY_LOG=$(LOG_LEVEL) \
	CLIPROXY_SNAPSHOT_URL=$(SNAPSHOT_URL) \
	CLIPROXY_SNAPSHOT_BEARER_TOKEN=$(SNAPSHOT_BEARER_TOKEN) \
	cargo run -- --bind-addr $(BIND_ADDR) --log-level $(LOG_LEVEL) --snapshot-url $(SNAPSHOT_URL) --snapshot-bearer-token $(SNAPSHOT_BEARER_TOKEN)

fmt:
	cargo fmt --all

check:
	cargo check --workspace

test:
	cargo test --workspace

build:
	cargo build --workspace

clean:
	cargo clean
