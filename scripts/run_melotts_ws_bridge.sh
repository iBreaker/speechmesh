#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  run_melotts_ws_bridge.sh [--python <python-bin>] [--listen <host:port>] [--upstream <url>]

Examples:
  run_melotts_ws_bridge.sh
  run_melotts_ws_bridge.sh --python /Users/breaker/src/MeloTTS/.venv/bin/python
  run_melotts_ws_bridge.sh --listen 127.0.0.1:8797 --upstream http://127.0.0.1:7797
EOF
}

python_bin="${PYTHON_BIN:-python3}"
listen_addr="${LISTEN_ADDR:-127.0.0.1:8797}"
upstream_url="${MELOTTS_BASE_URL:-http://127.0.0.1:7797}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --python)
      python_bin="$2"
      shift 2
      ;;
    --listen)
      listen_addr="$2"
      shift 2
      ;;
    --upstream)
      upstream_url="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

listen_host="${listen_addr%:*}"
listen_port="${listen_addr##*:}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "${python_bin}" "${script_dir}/melotts_ws_bridge.py" \
  --listen-host "${listen_host}" \
  --listen-port "${listen_port}" \
  --melotts-base-url "${upstream_url}"
