SHELL := /bin/bash

GO_CONFIG ?= temp/config.prod-auth-test.embedded.yaml
GO_ADDR ?= 127.0.0.1:8317
GO_HEALTH_URL ?= http://$(GO_ADDR)/healthz
MANAGEMENT_KEY ?= test-management-key
SNAPSHOT_URL ?= http://$(GO_ADDR)/v0/management/runtime-snapshot

RUST_BIND_ADDR ?= 127.0.0.1:4100
RUST_READY_URL ?= http://$(RUST_BIND_ADDR)/readyz
RUST_DIR ?= rust/cliproxy-data-plane
RUST_TARGET ?=
RELEASE_VERSION ?= dev
HOST_UNAME_S ?= $(shell uname -s)
HOST_UNAME_M ?= $(shell uname -m)
UPSTREAM_PROXY ?=
UPSTREAM_HTTP_PROXY ?=
UPSTREAM_HTTPS_PROXY ?=

TMP_DIR ?= temp
GO_DEV_BINARY ?= $(TMP_DIR)/cli-proxy-api-dev
GO_PID_FILE ?= $(TMP_DIR)/dev-go.pid
GO_LOG_FILE ?= $(TMP_DIR)/dev-go.log
RUST_PID_FILE ?= $(TMP_DIR)/dev-rust.pid
RUST_LOG_FILE ?= $(TMP_DIR)/dev-rust.log
EMBEDDED_STATE_DIR ?= $(TMP_DIR)/embedded-data-plane
EMBEDDED_RUST_STDOUT_LOG ?= $(EMBEDDED_STATE_DIR)/logs/data-plane/stdout.log
EMBEDDED_RUST_STDERR_LOG ?= $(EMBEDDED_STATE_DIR)/logs/data-plane/stderr.log
SNAPSHOT_FILE ?= $(TMP_DIR)/cpa-runtime-snapshot.dev.json
SNAPSHOT_CURRENT_FILE ?= $(TMP_DIR)/cpa-runtime-snapshot.current.json
RUST_SNAPSHOT_URL ?= http://$(RUST_BIND_ADDR)/v0/runtime/snapshot
RUST_SNAPSHOT_FILE ?= $(TMP_DIR)/rs-runtime-snapshot.current.json

.PHONY: help dev-stack stop-stack status-stack logs-stack logs-go logs-rust snapshot-current snapshot-rs diff-snapshots

help:
	@printf "%s\n" \
	"可用目标：" \
	"  make dev-stack      启动默认 embedded 本地联调栈（推荐，贴近生产）" \
	"  make stop-stack     停止 Go/Rust 联调进程" \
	"  make status-stack   查看 Go/Rust 联调进程状态" \
	"  make logs-stack     查看 Go/Rust 联调日志" \
	"  make logs-go        持续跟踪 Go 日志" \
	"  make logs-rust      持续跟踪 embedded Rust 子进程日志" \
	"  make snapshot-current 拉取当前 Go 管理面的原始 snapshot 到文件" \
	"  make snapshot-rs    拉取当前 Rust 数据面已应用的 snapshot 到文件" \
	"  make diff-snapshots 拉取 Go/Rust 当前 snapshot 并输出差异" \
	"" \
	"可选变量：" \
	"  GO_CONFIG=<path>        默认 $(GO_CONFIG)" \
	"  MANAGEMENT_KEY=<key>    默认 $(MANAGEMENT_KEY)" \
	"  GO_ADDR=<host:port>     默认 $(GO_ADDR)" \
	"  GO_DEV_BINARY=<path>    默认 $(GO_DEV_BINARY)" \
	"  RUST_BIND_ADDR=<addr>   默认 $(RUST_BIND_ADDR)" \
	"  SNAPSHOT_CURRENT_FILE=<path> 默认 $(SNAPSHOT_CURRENT_FILE)" \
	"  RUST_SNAPSHOT_FILE=<path> 默认 $(RUST_SNAPSHOT_FILE)" \
	"  RUST_TARGET=<triple>    可选；默认按当前机器推断，例如 aarch64-apple-darwin" \
	"  RELEASE_VERSION=<ver>   默认 $(RELEASE_VERSION)" \
	"  UPSTREAM_PROXY=<url|direct>  例如 socks5h://127.0.0.1:7897" \
	"  UPSTREAM_HTTP_PROXY=<url>  例如 http://127.0.0.1:7897" \
	"  UPSTREAM_HTTPS_PROXY=<url>  例如 http://127.0.0.1:7897"

dev-stack:
	@set -euo pipefail; \
	mkdir -p "$(TMP_DIR)"; \
	$(MAKE) stop-stack >/dev/null; \
	echo "编译 Go 管理平面..."; \
	go build -o "$(GO_DEV_BINARY)" ./cmd/server >/dev/null; \
	echo "编译 Rust 数据平面（debug）..."; \
	cargo build --manifest-path "$(RUST_DIR)/Cargo.toml" >/dev/null; \
	rust_binary_path="$(RUST_DIR)/target/debug/cliproxy-data-plane"; \
	case "$(HOST_UNAME_S)" in \
		MINGW64_NT-*|MSYS_NT-*|CYGWIN_NT-*) rust_binary_path="$$rust_binary_path.exe" ;; \
	esac; \
	echo "启动 Go 管理平面..."; \
	nohup env MANAGEMENT_PASSWORD="$(MANAGEMENT_KEY)" \
		CLIPROXY_DATA_PLANE_BINARY_PATH="$$rust_binary_path" \
		CLIPROXY_UPSTREAM_PROXY="$(UPSTREAM_PROXY)" CLIPROXY_UPSTREAM_HTTP_PROXY="$(UPSTREAM_HTTP_PROXY)" CLIPROXY_UPSTREAM_HTTPS_PROXY="$(UPSTREAM_HTTPS_PROXY)" \
		http_proxy="$${http_proxy-}" https_proxy="$${https_proxy-}" HTTP_PROXY="$${HTTP_PROXY-}" HTTPS_PROXY="$${HTTPS_PROXY-}" \
		"$(GO_DEV_BINARY)" --config "$(GO_CONFIG)" >"$(GO_LOG_FILE)" 2>&1 & echo $$! >"$(GO_PID_FILE)"; \
	for i in $$(seq 1 30); do \
		if curl -sf "$(GO_HEALTH_URL)" >/dev/null; then \
			break; \
		fi; \
		sleep 1; \
		if [[ $$i -eq 30 ]]; then \
			echo "Go 管理平面启动超时，查看日志: $(GO_LOG_FILE)"; \
			exit 1; \
		fi; \
	done; \
	for i in $$(seq 1 30); do \
		if curl -sf "$(RUST_READY_URL)" >/dev/null; then \
			echo "embedded 联调栈已就绪"; \
			echo "Go  日志: $(GO_LOG_FILE)"; \
			echo "Rust 日志: $(EMBEDDED_RUST_STDOUT_LOG) / $(EMBEDDED_RUST_STDERR_LOG)"; \
			exit 0; \
		fi; \
		sleep 1; \
		if [[ $$i -eq 30 ]]; then \
			echo "embedded Rust 数据平面启动超时，查看日志: $(EMBEDDED_RUST_STDOUT_LOG)"; \
			exit 1; \
		fi; \
	done

snapshot-current:
	@set -euo pipefail; \
	mkdir -p "$(dir $(SNAPSHOT_CURRENT_FILE))"; \
	curl -sf "$(SNAPSHOT_URL)" \
		-H "Authorization: Bearer $(MANAGEMENT_KEY)" \
		-o "$(SNAPSHOT_CURRENT_FILE)"; \
	python3 -c 'import json, pathlib; path = pathlib.Path("$(SNAPSHOT_CURRENT_FILE)"); data = json.loads(path.read_text()); path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n"); print(path)'

snapshot-rs:
	@set -euo pipefail; \
	mkdir -p "$(dir $(RUST_SNAPSHOT_FILE))"; \
	curl -sf "$(RUST_SNAPSHOT_URL)" \
		-o "$(RUST_SNAPSHOT_FILE)"; \
	python3 -c 'import json, pathlib; path = pathlib.Path("$(RUST_SNAPSHOT_FILE)"); data = json.loads(path.read_text()); path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n"); print(path)'

diff-snapshots:
	@set -euo pipefail; \
	$(MAKE) snapshot-current >/dev/null; \
	$(MAKE) snapshot-rs >/dev/null; \
	echo "Go snapshot:   $(SNAPSHOT_CURRENT_FILE)"; \
	echo "Rust snapshot: $(RUST_SNAPSHOT_FILE)"; \
	python3 -c 'import difflib, pathlib, sys; go = pathlib.Path("$(SNAPSHOT_CURRENT_FILE)").read_text().splitlines(); rs = pathlib.Path("$(RUST_SNAPSHOT_FILE)").read_text().splitlines(); diff = list(difflib.unified_diff(go, rs, fromfile="$(SNAPSHOT_CURRENT_FILE)", tofile="$(RUST_SNAPSHOT_FILE)", lineterm="")); print("\n".join(diff) if diff else "No differences.")'

stop-stack:
	@set -euo pipefail; \
	go_port="$${GO_ADDR##*:}"; \
	rust_port="$${RUST_BIND_ADDR##*:}"; \
	for port in "$$rust_port" "$$go_port"; do \
		for pid in $$(lsof -tiTCP:$$port -sTCP:LISTEN 2>/dev/null || true); do \
			kill $$pid 2>/dev/null || true; \
		done; \
	done; \
	sleep 1; \
	for file in "$(RUST_PID_FILE)" "$(GO_PID_FILE)"; do \
		if [[ -f $$file ]]; then \
			pid=$$(cat $$file); \
			if kill -0 $$pid 2>/dev/null; then \
				kill $$pid 2>/dev/null || true; \
				wait $$pid 2>/dev/null || true; \
			fi; \
			rm -f $$file; \
		fi; \
	done; \
	for port in "$$go_port" "$$rust_port"; do \
		for pid in $$(lsof -tiTCP:$$port -sTCP:LISTEN 2>/dev/null || true); do \
			kill $$pid 2>/dev/null || true; \
		done; \
	done; \
	GO_ADDR="$(GO_ADDR)" RUST_BIND_ADDR="$(RUST_BIND_ADDR)" ./scripts/dev-stack-procs.sh --kill >/dev/null || true

status-stack:
	@set -euo pipefail; \
	go_port="$${GO_ADDR##*:}"; \
	go_pid="$$(lsof -tiTCP:$$go_port -sTCP:LISTEN 2>/dev/null | head -n 1 || true)"; \
	if [[ -n "$$go_pid" ]]; then \
		echo "Go 运行中 pid=$$go_pid"; \
		curl -sf "$(GO_HEALTH_URL)" || true; \
		echo; \
	else \
		echo "Go 未运行"; \
	fi; \
	rust_port="$${RUST_BIND_ADDR##*:}"; \
	rust_pid="$$(lsof -tiTCP:$$rust_port -sTCP:LISTEN 2>/dev/null | head -n 1 || true)"; \
	if [[ -n "$$rust_pid" ]]; then \
		echo "Rust 运行中 pid=$$rust_pid"; \
		curl -sf "$(RUST_READY_URL)" || true; \
		echo; \
	else \
		echo "Rust 未运行"; \
	fi

logs-stack:
	@set -euo pipefail; \
	for file in "$(GO_LOG_FILE)" "$(EMBEDDED_RUST_STDOUT_LOG)" "$(EMBEDDED_RUST_STDERR_LOG)" "$(RUST_LOG_FILE)"; do \
		if [[ -f $$file ]]; then \
			echo "===== $$file ====="; \
			tail -n 40 $$file; \
		else \
			echo "日志不存在: $$file"; \
		fi; \
	done

logs-go:
	@if [[ ! -f "$(GO_LOG_FILE)" ]]; then \
		echo "Go 日志不存在: $(GO_LOG_FILE)"; \
		echo "先执行 make dev-stack"; \
		exit 1; \
	fi
	@echo "跟踪 Go 日志: $(GO_LOG_FILE)"
	@tail -f "$(GO_LOG_FILE)"

logs-rust:
	@if [[ ! -f "$(EMBEDDED_RUST_STDOUT_LOG)" && ! -f "$(EMBEDDED_RUST_STDERR_LOG)" ]]; then \
		echo "embedded Rust 日志不存在: $(EMBEDDED_RUST_STDOUT_LOG) / $(EMBEDDED_RUST_STDERR_LOG)"; \
		echo "先执行 make dev-stack"; \
		exit 1; \
	fi
	@echo "跟踪 embedded Rust 日志: $(EMBEDDED_RUST_STDOUT_LOG) $(EMBEDDED_RUST_STDERR_LOG)"
	@tail -f "$(EMBEDDED_RUST_STDOUT_LOG)" "$(EMBEDDED_RUST_STDERR_LOG)"
