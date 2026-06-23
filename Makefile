SHELL := /bin/bash

GO_CONFIG ?= temp/config.prod-auth-test.yaml
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
GO_PID_FILE ?= $(TMP_DIR)/dev-go.pid
GO_LOG_FILE ?= $(TMP_DIR)/dev-go.log
RUST_PID_FILE ?= $(TMP_DIR)/dev-rust.pid
RUST_LOG_FILE ?= $(TMP_DIR)/dev-rust.log
SNAPSHOT_FILE ?= $(TMP_DIR)/cpa-runtime-snapshot.dev.json
SNAPSHOT_CURRENT_FILE ?= $(TMP_DIR)/cpa-runtime-snapshot.current.json
RUST_SNAPSHOT_URL ?= http://$(RUST_BIND_ADDR)/v0/runtime/snapshot
RUST_SNAPSHOT_FILE ?= $(TMP_DIR)/rs-runtime-snapshot.current.json

.PHONY: help dev-stack dev-stack-url stop-stack restart-stack status-stack ps-stack kill-stack-orphans logs-stack logs-go logs-rust snapshot-stack snapshot-current snapshot-rs diff-snapshots test-responses prepare-embedded-data-plane build-release-embedded clean-release-embedded

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
	"  make snapshot-current 拉取当前 Go 管理面的原始 snapshot 到文件" \
	"  make snapshot-rs    拉取当前 Rust 数据面已应用的 snapshot 到文件" \
	"  make diff-snapshots 拉取 Go/Rust 当前 snapshot 并输出差异" \
	"  make test-responses 调用 Rust /v1/responses 测试接口" \
	"  make prepare-embedded-data-plane 生成 release 用 embedded Rust artifact" \
	"  make build-release-embedded       用 release_embedded_artifact tag 构建 Go 主程序" \
	"  make clean-release-embedded       清理临时生成的 embedded artifact 文件" \
	"" \
	"可选变量：" \
	"  GO_CONFIG=<path>        默认 $(GO_CONFIG)" \
	"  MANAGEMENT_KEY=<key>    默认 $(MANAGEMENT_KEY)" \
	"  GO_ADDR=<host:port>     默认 $(GO_ADDR)" \
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
		CLIPROXY_UPSTREAM_PROXY="$(UPSTREAM_PROXY)" CLIPROXY_UPSTREAM_HTTP_PROXY="$(UPSTREAM_HTTP_PROXY)" CLIPROXY_UPSTREAM_HTTPS_PROXY="$(UPSTREAM_HTTPS_PROXY)" \
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
		CLIPROXY_UPSTREAM_PROXY="$(UPSTREAM_PROXY)" CLIPROXY_UPSTREAM_HTTP_PROXY="$(UPSTREAM_HTTP_PROXY)" CLIPROXY_UPSTREAM_HTTPS_PROXY="$(UPSTREAM_HTTPS_PROXY)" \
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

prepare-embedded-data-plane:
	@set -euo pipefail; \
	rust_target="$(RUST_TARGET)"; \
	if [[ -z "$$rust_target" ]]; then \
		case "$(HOST_UNAME_S):$(HOST_UNAME_M)" in \
			Darwin:arm64) rust_target="aarch64-apple-darwin" ;; \
			Darwin:x86_64) rust_target="x86_64-apple-darwin" ;; \
			Linux:arm64|Linux:aarch64) rust_target="aarch64-unknown-linux-gnu" ;; \
			Linux:x86_64) rust_target="x86_64-unknown-linux-gnu" ;; \
			MINGW64_NT-*:x86_64|MSYS_NT-*:x86_64) rust_target="x86_64-pc-windows-msvc" ;; \
			MINGW64_NT-*:arm64|MSYS_NT-*:arm64) rust_target="aarch64-pc-windows-msvc" ;; \
			*) \
				echo "Unable to infer RUST_TARGET from $(HOST_UNAME_S)/$(HOST_UNAME_M); please set it explicitly."; \
				exit 1; \
				;; \
		esac; \
	fi; \
	RUST_TARGET="$$rust_target" RELEASE_VERSION="$(RELEASE_VERSION)" bash ./scripts/prepare-embedded-data-plane-release.sh

build-release-embedded:
	@set -euo pipefail; \
	go build -tags release_embedded_artifact -o cli-proxy-api ./cmd/server

clean-release-embedded:
	@rm -f internal/dataplane/embedded/release_artifact.bin internal/dataplane/embedded/release_artifact_generated.go
