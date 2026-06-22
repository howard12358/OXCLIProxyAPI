#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
go_addr="${GO_ADDR:-127.0.0.1:8317}"
rust_addr="${RUST_BIND_ADDR:-127.0.0.1:4100}"
go_port="${go_addr##*:}"
rust_port="${rust_addr##*:}"
kill_mode="${1:-}"
tmp_file="$(mktemp)"
trap 'rm -f "$tmp_file"' EXIT

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

while IFS= read -r line; do
  [[ -n "$line" ]] || continue
  pid="${line%% *}"
  rest="${line#* }"
  ppid="${rest%% *}"
  command="${rest#* }"

  if [[ "$command" == *"$repo_root"* ]]; then
    if [[ "$command" == *"cliproxy-data-plane"* ]]; then
      record_pid "$pid" "rust" "cmdline" "$ppid" "$command"
      continue
    fi
    if [[ "$command" == *"/server --config"* || "$command" == *"go run ./cmd/server"* || "$command" == *"/cmd/server"* ]]; then
      record_pid "$pid" "go" "cmdline" "$ppid" "$command"
      continue
    fi
  fi
done < <(ps ax -o pid=,ppid=,command=)

for port in "$go_port" "$rust_port"; do
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    line="$(ps -p "$pid" -o pid=,ppid=,command= 2>/dev/null || true)"
    [[ -n "$line" ]] || continue
    _pid="${line%% *}"
    rest="${line#* }"
    ppid="${rest%% *}"
    command="${rest#* }"
    kind="unknown"
    if [[ "$port" == "$go_port" ]]; then
      kind="go"
    elif [[ "$port" == "$rust_port" ]]; then
      kind="rust"
    fi
    record_pid "$_pid" "$kind" "listen:${port}" "$ppid" "$command"
  done < <(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)
done

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
