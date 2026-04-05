#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  install_apple_agent_service.sh [install|restart|stop|status|uninstall] [options]

Options:
  --skip-build             Skip rebuilding binaries
  --gateway-url URL        Gateway URL, default wss://speechmesh.example.com/agent
  --agent-id ID            Agent ID, default apple-agent-1
  --agent-name NAME        Agent name, default "Apple ASR Agent"
  --shared-secret SECRET   Shared secret used by gateway and agent
USAGE
}

action="install"
skip_build="false"
gateway_url="wss://speechmesh.example.com/agent"
agent_id="apple-agent-1"
agent_name="Apple ASR Agent"
shared_secret="change-me"

while [[ $# -gt 0 ]]; do
  case "$1" in
    install|restart|stop|status|uninstall)
      action="$1"
      shift
      ;;
    --skip-build)
      skip_build="true"
      shift
      ;;
    --gateway-url)
      gateway_url="$2"
      shift 2
      ;;
    --agent-id)
      agent_id="$2"
      shift 2
      ;;
    --agent-name)
      agent_name="$2"
      shift 2
      ;;
    --shared-secret)
      shared_secret="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
template_plist="${repo_root}/deploy/macos/io.speechmesh.apple-agent.plist"
plist_dst="${HOME}/Library/LaunchAgents/io.speechmesh.apple-agent.plist"
label="io.speechmesh.apple-agent"
uid="$(id -u)"

agent_bin="${repo_root}/target/release/apple_agent"
apple_bridge_bin="${repo_root}/bridges/apple-asr/.build/release/apple-asr-bridge"

escape_path_for_sed() {
  printf '%s' "$1" | sed 's/[&]/\\&/g'
}

render_plist() {
  mkdir -p "${HOME}/Library/LaunchAgents" "${HOME}/Library/Logs/SpeechMesh"
  local escaped_repo escaped_home escaped_gateway escaped_agent_id escaped_agent_name escaped_secret
  escaped_repo="$(escape_path_for_sed "${repo_root}")"
  escaped_home="$(escape_path_for_sed "${HOME}")"
  escaped_gateway="$(escape_path_for_sed "${gateway_url}")"
  escaped_agent_id="$(escape_path_for_sed "${agent_id}")"
  escaped_agent_name="$(escape_path_for_sed "${agent_name}")"
  escaped_secret="$(escape_path_for_sed "${shared_secret}")"
  sed \
    -e "s|__REPO_ROOT__|${escaped_repo}|g" \
    -e "s|__HOME__|${escaped_home}|g" \
    -e "s|__GATEWAY_URL__|${escaped_gateway}|g" \
    -e "s|__AGENT_ID__|${escaped_agent_id}|g" \
    -e "s|__AGENT_NAME__|${escaped_agent_name}|g" \
    -e "s|__SHARED_SECRET__|${escaped_secret}|g" \
    "${template_plist}" > "${plist_dst}"
  plutil -lint "${plist_dst}" >/dev/null
}

build_binaries() {
  if [[ "${skip_build}" == "true" ]]; then
    return
  fi
  cargo build -p speechmeshd --release --bin apple_agent
  (
    cd "${repo_root}/bridges/apple-asr"
    swift build -c release
  )
}

ensure_binaries_exist() {
  if [[ ! -x "${agent_bin}" ]]; then
    echo "missing apple_agent binary: ${agent_bin}" >&2
    exit 1
  fi
  if [[ ! -x "${apple_bridge_bin}" ]]; then
    echo "missing apple-asr-bridge binary: ${apple_bridge_bin}" >&2
    exit 1
  fi
}

bootout_service() {
  launchctl bootout "gui/${uid}" "${plist_dst}" >/dev/null 2>&1 || true
}

bootstrap_service() {
  launchctl bootstrap "gui/${uid}" "${plist_dst}"
  launchctl kickstart -k "gui/${uid}/${label}"
}

status_service() {
  launchctl print "gui/${uid}/${label}" | sed -n '1,120p'
}

install_service() {
  build_binaries
  ensure_binaries_exist
  render_plist
  bootout_service
  bootstrap_service
  status_service
}

restart_service() {
  install_service
}

stop_service() {
  bootout_service
}

uninstall_service() {
  bootout_service
  rm -f "${plist_dst}"
}

case "${action}" in
  install)
    install_service
    ;;
  restart)
    restart_service
    ;;
  stop)
    stop_service
    ;;
  status)
    status_service
    ;;
  uninstall)
    uninstall_service
    ;;
esac
