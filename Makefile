SHELL := /bin/bash

GO_CONFIG ?= temp/config.prod-auth-test.yaml
GO_ADDR ?= 127.0.0.1:8317
GO_HEALTH_URL ?= http://$(GO_ADDR)/healthz
MANAGEMENT_KEY ?= test-management-key
SNAPSHOT_URL ?= http://$(GO_ADDR)/v0/management/runtime-snapshot

RUST_BIND_ADDR ?= 127.0.0.1:4100
RUST_READY_URL ?= http://$(RUST_BIND_ADDR)/readyz
RUST_DIR ?= rust/cliproxy-data-plane
UPSTREAM_HTTP_PROXY ?=
UPSTREAM_HTTPS_PROXY ?=

TMP_DIR ?= temp
GO_PID_FILE ?= $(TMP_DIR)/dev-go.pid
GO_LOG_FILE ?= $(TMP_DIR)/dev-go.log
RUST_PID_FILE ?= $(TMP_DIR)/dev-rust.pid
RUST_LOG_FILE ?= $(TMP_DIR)/dev-rust.log
SNAPSHOT_FILE ?= $(TMP_DIR)/cpa-runtime-snapshot.dev.json

.PHONY: help dev-stack stop-stack restart-stack status-stack ps-stack kill-stack-orphans logs-stack logs-go logs-rust snapshot-stack test-responses

help:
	@printf "%s\n" \
	"可用目标：" \
	"  make dev-stack      启动 Go 管理平面和 Rust 数据平面" \
	"  make dev-stack-url  启动 Go 管理平面，并让 Rust 通过网络持续拉取 snapshot" \
	"  make stop-stack     停止 Go/Rust 联调进程" \
	"  make restart-stack  重启 Go/Rust 联调进程" \
	"  make status-stack   查看 Go/Rust 联调进程状态" \
	"  make ps-stack       列出当前仓库相关的 Go/Rust 进程" \
	"  make kill-stack-orphans 杀掉当前仓库相关的 Go/Rust 孤儿进程" \
	"  make logs-stack     查看 Go/Rust 联调日志" \
	"  make logs-go        持续跟踪 Go 日志" \
	"  make logs-rust      持续跟踪 Rust 日志" \
	"  make snapshot-stack 重新导出本地 snapshot 文件" \
	"  make test-responses 调用 Rust /v1/responses 测试接口" \
	"" \
	"可选变量：" \
	"  GO_CONFIG=<path>        默认 $(GO_CONFIG)" \
	"  MANAGEMENT_KEY=<key>    默认 $(MANAGEMENT_KEY)" \
	"  GO_ADDR=<host:port>     默认 $(GO_ADDR)" \
	"  RUST_BIND_ADDR=<addr>   默认 $(RUST_BIND_ADDR)" \
	"  UPSTREAM_HTTP_PROXY=<url>  例如 http://127.0.0.1:7897" \
	"  UPSTREAM_HTTPS_PROXY=<url>  例如 http://127.0.0.1:7897"

dev-stack:
	@set -euo pipefail; \
	mkdir -p "$(TMP_DIR)"; \
	$(MAKE) stop-stack >/dev/null; \
	echo "启动 Go 管理平面..."; \
	nohup go run ./cmd/server --config "$(GO_CONFIG)" >"$(GO_LOG_FILE)" 2>&1 & echo $$! >"$(GO_PID_FILE)"; \
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
	$(MAKE) snapshot-stack >/dev/null; \
	echo "启动 Rust 数据平面..."; \
	nohup env http_proxy="$${http_proxy-}" https_proxy="$${https_proxy-}" HTTP_PROXY="$${HTTP_PROXY-}" HTTPS_PROXY="$${HTTPS_PROXY-}" all_proxy= ALL_PROXY= \
		CLIPROXY_UPSTREAM_HTTP_PROXY="$(UPSTREAM_HTTP_PROXY)" CLIPROXY_UPSTREAM_HTTPS_PROXY="$(UPSTREAM_HTTPS_PROXY)" \
		cargo run --manifest-path "$(RUST_DIR)/Cargo.toml" -- --bind-addr "$(RUST_BIND_ADDR)" --snapshot-file "$(SNAPSHOT_FILE)" \
		>"$(RUST_LOG_FILE)" 2>&1 & echo $$! >"$(RUST_PID_FILE)"; \
	for i in $$(seq 1 30); do \
		if curl -sf "$(RUST_READY_URL)" >/dev/null; then \
			echo "联调栈已就绪"; \
			echo "Go  日志: $(GO_LOG_FILE)"; \
			echo "Rust 日志: $(RUST_LOG_FILE)"; \
			exit 0; \
		fi; \
		sleep 1; \
		if [[ $$i -eq 30 ]]; then \
			echo "Rust 数据平面启动超时，查看日志: $(RUST_LOG_FILE)"; \
			exit 1; \
		fi; \
	done

dev-stack-url:
	@set -euo pipefail; \
	mkdir -p "$(TMP_DIR)"; \
	$(MAKE) stop-stack >/dev/null; \
	echo "启动 Go 管理平面..."; \
	nohup go run ./cmd/server --config "$(GO_CONFIG)" >"$(GO_LOG_FILE)" 2>&1 & echo $$! >"$(GO_PID_FILE)"; \
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
	echo "启动 Rust 数据平面（snapshot-url 模式）..."; \
	nohup env http_proxy="$${http_proxy-}" https_proxy="$${https_proxy-}" HTTP_PROXY="$${HTTP_PROXY-}" HTTPS_PROXY="$${HTTPS_PROXY-}" all_proxy= ALL_PROXY= \
		CLIPROXY_UPSTREAM_HTTP_PROXY="$(UPSTREAM_HTTP_PROXY)" CLIPROXY_UPSTREAM_HTTPS_PROXY="$(UPSTREAM_HTTPS_PROXY)" \
		cargo run --manifest-path "$(RUST_DIR)/Cargo.toml" -- --bind-addr "$(RUST_BIND_ADDR)" --snapshot-url "$(SNAPSHOT_URL)" --snapshot-bearer-token "$(MANAGEMENT_KEY)" \
		>"$(RUST_LOG_FILE)" 2>&1 & echo $$! >"$(RUST_PID_FILE)"; \
	for i in $$(seq 1 30); do \
		if curl -sf "$(RUST_READY_URL)" >/dev/null; then \
			echo "联调栈已就绪（snapshot-url 模式）"; \
			echo "Go  日志: $(GO_LOG_FILE)"; \
			echo "Rust 日志: $(RUST_LOG_FILE)"; \
			exit 0; \
		fi; \
		sleep 1; \
		if [[ $$i -eq 30 ]]; then \
			echo "Rust 数据平面启动超时，查看日志: $(RUST_LOG_FILE)"; \
			exit 1; \
		fi; \
	done

snapshot-stack:
	@set -euo pipefail; \
	mkdir -p "$(TMP_DIR)"; \
	curl -sf "$(SNAPSHOT_URL)" \
		-H "Authorization: Bearer $(MANAGEMENT_KEY)" \
		-o "$(SNAPSHOT_FILE)"; \
	python3 -c 'import json, pathlib; path = pathlib.Path("$(SNAPSHOT_FILE)"); data = json.loads(path.read_text()); data.setdefault("listeners", {})["public_http"] = "http://$(RUST_BIND_ADDR)"; path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n"); print(path)'

stop-stack:
	@set -euo pipefail; \
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
	go_port="$${GO_ADDR##*:}"; \
	rust_port="$${RUST_BIND_ADDR##*:}"; \
	for port in "$$go_port" "$$rust_port"; do \
		for pid in $$(lsof -tiTCP:$$port -sTCP:LISTEN 2>/dev/null || true); do \
			kill $$pid 2>/dev/null || true; \
		done; \
	done; \
	GO_ADDR="$(GO_ADDR)" RUST_BIND_ADDR="$(RUST_BIND_ADDR)" ./scripts/dev-stack-procs.sh --kill >/dev/null || true

restart-stack:
	@$(MAKE) stop-stack
	@$(MAKE) dev-stack

status-stack:
	@set -euo pipefail; \
	for name in "Go:$(GO_PID_FILE):$(GO_HEALTH_URL)" "Rust:$(RUST_PID_FILE):$(RUST_READY_URL)"; do \
		service=$${name%%:*}; \
		rest=$${name#*:}; \
		pid_file=$${rest%%:*}; \
		url=$${rest##*:}; \
		if [[ -f $$pid_file ]]; then \
			pid=$$(cat $$pid_file); \
			if kill -0 $$pid 2>/dev/null; then \
				echo "$$service 运行中 pid=$$pid"; \
				curl -sf "$$url" || true; \
				echo; \
			else \
				echo "$$service pid 文件存在但进程已退出"; \
			fi; \
		else \
			echo "$$service 未运行"; \
		fi; \
	done

ps-stack:
	@GO_ADDR="$(GO_ADDR)" RUST_BIND_ADDR="$(RUST_BIND_ADDR)" ./scripts/dev-stack-procs.sh

kill-stack-orphans:
	@GO_ADDR="$(GO_ADDR)" RUST_BIND_ADDR="$(RUST_BIND_ADDR)" ./scripts/dev-stack-procs.sh --kill

logs-stack:
	@set -euo pipefail; \
	for file in "$(GO_LOG_FILE)" "$(RUST_LOG_FILE)"; do \
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
	@if [[ ! -f "$(RUST_LOG_FILE)" ]]; then \
		echo "Rust 日志不存在: $(RUST_LOG_FILE)"; \
		echo "先执行 make dev-stack"; \
		exit 1; \
	fi
	@echo "跟踪 Rust 日志: $(RUST_LOG_FILE)"
	@tail -f "$(RUST_LOG_FILE)"

test-responses:
	@curl -sS -N "http://$(RUST_BIND_ADDR)/v1/responses" \
		-H 'content-type: application/json' \
		-d '{"model":"gpt-5.5","stream":true,"input":"hello from make dev-stack"}'
