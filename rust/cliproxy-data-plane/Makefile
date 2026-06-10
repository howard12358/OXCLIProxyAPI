APP_NAME := cliproxy-data-plane
BIND_ADDR ?= 127.0.0.1:4100
LOG_LEVEL ?= info
SNAPSHOT_FILE ?= examples/runtime-snapshot.example.json

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
	"  SNAPSHOT_FILE=<path>   默认 $(SNAPSHOT_FILE)"

run:
	CLIPROXY_BIND=$(BIND_ADDR) \
	CLIPROXY_LOG=$(LOG_LEVEL) \
	CLIPROXY_SNAPSHOT_FILE=$(SNAPSHOT_FILE) \
	cargo run -- --bind-addr $(BIND_ADDR) --log-level $(LOG_LEVEL) --snapshot-file $(SNAPSHOT_FILE)

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
