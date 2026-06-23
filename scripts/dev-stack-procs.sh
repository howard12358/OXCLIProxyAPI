#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
go_addr="${GO_ADDR:-127.0.0.1:8317}"
rust_addr="${RUST_BIND_ADDR:-127.0.0.1:4100}"
go_port="${go_addr##*:}"
rust_port="${rust_addr##*:}"
tmp_dir="${TMP_DIR:-temp}"
go_pid_file="${GO_PID_FILE:-$tmp_dir/dev-go.pid}"
rust_pid_file="${RUST_PID_FILE:-$tmp_dir/dev-rust.pid}"
kill_mode="${1:-}"
tmp_file="$(mktemp)"
trap 'rm -f "$tmp_file"' EXIT

to_lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

parse_ps_line() {
  local line="$1"
  local __pid_var="$2"
  local __ppid_var="$3"
  local __command_var="$4"
  local parsed_pid=""
  local parsed_ppid=""
  local parsed_command=""

  read -r parsed_pid parsed_ppid parsed_command <<<"$(printf '%s\n' "$line" | sed -E 's/^[[:space:]]+//')"

  printf -v "$__pid_var" '%s' "$parsed_pid"
  printf -v "$__ppid_var" '%s' "$parsed_ppid"
  printf -v "$__command_var" '%s' "$parsed_command"
}

record_pid() {
  local pid="$1"
  local kind="$2"
  local reason="$3"
  local ppid="$4"
  local command="$5"

  [[ -n "$pid" ]] || return 0
  [[ "$pid" =~ ^[0-9]+$ ]] || return 0

  printf "%s\t%s\t%s\t%s\t%s\n" "$pid" "$ppid" "$kind" "$reason" "$command" >>"$tmp_file"
}

for port in "$go_port" "$rust_port"; do
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    line="$(ps -p "$pid" -o pid=,ppid=,command= 2>/dev/null || true)"
    [[ -n "$line" ]] || continue
    parse_ps_line "$line" _pid ppid command
    kind="unknown"
    if [[ "$port" == "$go_port" ]]; then
      kind="go"
    elif [[ "$port" == "$rust_port" ]]; then
      kind="rust"
    fi
    record_pid "$_pid" "$kind" "listen:${port}" "$ppid" "$command"
  done < <(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)
done

for spec in \
  "go:$go_pid_file" \
  "rust:$rust_pid_file"; do
  kind="${spec%%:*}"
  pid_file="${spec#*:}"
  [[ -f "$pid_file" ]] || continue
  pid="$(cat "$pid_file" 2>/dev/null || true)"
  [[ -n "$pid" ]] || continue
  line="$(ps -p "$pid" -o pid=,ppid=,command= 2>/dev/null || true)"
  [[ -n "$line" ]] || continue
  parse_ps_line "$line" _pid ppid command
  record_pid "$_pid" "$kind" "pidfile:$(basename "$pid_file")" "$ppid" "$command"
done

while IFS= read -r line; do
  [[ -n "$line" ]] || continue
  parse_ps_line "$line" pid ppid command
  [[ -n "${command:-}" ]] || continue

  if [[ "$(to_lower "$command")" == *"$(to_lower "$repo_root")"* ]]; then
    if [[ "$command" == *"cliproxy-data-plane"* ]]; then
      record_pid "$pid" "rust" "cmdline" "$ppid" "$command"
    fi
  fi
done < <(pgrep -fl "cliproxy-data-plane" 2>/dev/null || true)

while IFS= read -r line; do
  [[ -n "$line" ]] || continue
  parse_ps_line "$line" pid ppid command
  [[ -n "${command:-}" ]] || continue

  if [[ "$(to_lower "$command")" == *"$(to_lower "$repo_root")"* ]]; then
    if [[ "$command" == *"/server --config"* || "$command" == *"go run ./cmd/server"* || "$command" == *"/cmd/server"* ]]; then
      record_pid "$pid" "go" "cmdline" "$ppid" "$command"
    fi
  fi
done < <(pgrep -fl "/server --config|go run ./cmd/server|/cmd/server" 2>/dev/null || true)

if [[ ! -s "$tmp_file" ]]; then
  echo "未发现当前仓库相关的 Go/Rust 进程"
  exit 0
fi

awk -F '\t' '
  {
    pid = $1
    if (!(pid in seen)) {
      seen[pid] = 1
      order[++count] = pid
      ppid[pid] = $2
      kind[pid] = $3
      reason[pid] = $4
      command[pid] = $5
      next
    }
    if (index("," reason[pid] ",", "," $4 ",") == 0) {
      reason[pid] = reason[pid] "," $4
    }
  }
  END {
    for (i = 1; i <= count; i++) {
      pid = order[i]
      printf "%s\t%s\t%s\t%s\t%s\n", pid, ppid[pid], kind[pid], reason[pid], command[pid]
    }
  }
' "$tmp_file" | sort -n -k1,1 | awk -F '\t' '
  BEGIN {
    printf "%-8s %-8s %-8s %-18s %s\n", "PID", "PPID", "TYPE", "SOURCE", "COMMAND"
  }
  {
    printf "%-8s %-8s %-8s %-18s %s\n", $1, $2, $3, $4, $5
  }
'

if [[ "$kill_mode" == "--kill" ]]; then
  echo
  awk -F '\t' '!seen[$1]++ {print $1}' "$tmp_file" | sort -n | while IFS= read -r pid; do
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      echo "已发送终止信号 pid=$pid"
    fi
  done
fi
