#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  install_device_agent_service.sh [install|restart|stop|status|uninstall] [options]

Options:
  --platform PLATFORM      auto|macos|linux, default auto
  --skip-build             Skip rebuilding the speechmesh binary
  --skip-install-binary    Skip copying binary into --binary-path
  --binary-path PATH       Installed speechmesh path, default ~/bin/speechmesh
  --gateway-url URL        Gateway URL, default wss://speechmesh.example.com/agent
  --agent-id ID            Agent ID, default device-agent-1
  --agent-name NAME        Agent name, default "SpeechMesh Device Agent"
  --device-id ID           Device ID, default local-device
  --shared-secret SECRET   Shared secret used by gateway and agent
  --legacy-compat MODE     keep|wrap|remove old speechmesh-cli/speechmesh-agent wrappers, default wrap
  --service-name NAME      Linux systemd --user service name, default speechmesh-device-agent
  --update-manifest-url URL  Enable auto-update checks from this manifest
  --update-channel NAME      Auto-update channel, default stable
  --update-interval-secs N   Auto-update polling interval, default 300
  --update-status-path PATH  Auto-update state JSON path
  --disable-auto-update      Do not install updater scheduler assets
USAGE
}

action="install"
platform="auto"
skip_build="false"
skip_install_binary="false"
binary_path="${HOME}/bin/speechmesh"
gateway_url="wss://speechmesh.example.com/agent"
agent_id="device-agent-1"
agent_name="SpeechMesh Device Agent"
device_id="local-device"
shared_secret="change-me"
legacy_compat="wrap"
service_name="speechmesh-device-agent"
update_manifest_url=""
update_channel="stable"
update_interval_secs="300"
update_status_path=""
disable_auto_update="false"

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
    --platform)
      platform="$2"
      shift 2
      ;;
    --skip-install-binary)
      skip_install_binary="true"
      shift
      ;;
    --binary-path)
      binary_path="$2"
      shift 2
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
    --device-id)
      device_id="$2"
      shift 2
      ;;
    --shared-secret)
      shared_secret="$2"
      shift 2
      ;;
    --legacy-compat)
      legacy_compat="$2"
      shift 2
      ;;
    --service-name)
      service_name="$2"
      shift 2
      ;;
    --update-manifest-url)
      update_manifest_url="$2"
      shift 2
      ;;
    --update-channel)
      update_channel="$2"
      shift 2
      ;;
    --update-interval-secs)
      update_interval_secs="$2"
      shift 2
      ;;
    --update-status-path)
      update_status_path="$2"
      shift 2
      ;;
    --disable-auto-update)
      disable_auto_update="true"
      shift
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
binary_dir="$(dirname "${binary_path}")"
built_binary="${repo_root}/target/release/speechmesh"
template_plist="${repo_root}/deploy/macos/io.speechmesh.device-agent.plist"
plist_dst="${HOME}/Library/LaunchAgents/io.speechmesh.device-agent.plist"
launchd_label="io.speechmesh.device-agent"
template_updater_plist="${repo_root}/deploy/macos/io.speechmesh.device-agent-updater.plist"
updater_plist_dst="${HOME}/Library/LaunchAgents/io.speechmesh.device-agent-updater.plist"
updater_launchd_label="io.speechmesh.device-agent-updater"
uid="$(id -u)"
template_unit="${repo_root}/deploy/linux/speechmesh-device-agent.service"
unit_dst="${HOME}/.config/systemd/user/${service_name}.service"
template_updater_unit="${repo_root}/deploy/linux/speechmesh-device-agent-updater.service"
template_updater_timer="${repo_root}/deploy/linux/speechmesh-device-agent-updater.timer"
updater_service_name="${service_name}-updater"
updater_unit_dst="${HOME}/.config/systemd/user/${updater_service_name}.service"
updater_timer_dst="${HOME}/.config/systemd/user/${updater_service_name}.timer"
escaped_repo=""
escaped_home=""
escaped_binary=""
escaped_gateway=""
escaped_agent_id=""
escaped_agent_name=""
escaped_device_id=""
escaped_secret=""
escaped_update_manifest=""
escaped_update_channel=""
escaped_update_interval=""
escaped_update_status=""
escaped_service_name=""

detect_platform() {
  if [[ "${platform}" == "auto" ]]; then
    case "$(uname -s)" in
      Darwin)
        platform="macos"
        ;;
      Linux)
        platform="linux"
        ;;
      *)
        echo "unsupported platform: $(uname -s)" >&2
        exit 1
        ;;
    esac
  fi
  if [[ "${platform}" != "macos" && "${platform}" != "linux" ]]; then
    echo "invalid --platform value: ${platform}" >&2
    exit 1
  fi
  if [[ -z "${update_status_path}" ]]; then
    if [[ "${platform}" == "macos" ]]; then
      update_status_path="${HOME}/Library/Application Support/SpeechMesh/device-agent-update.json"
    else
      update_status_path="${HOME}/.local/state/speechmesh/device-agent-update.json"
    fi
  fi
}

escape_path_for_sed() {
  printf '%s' "$1" | sed 's/[&]/\\&/g'
}

render_common_subst() {
  escaped_repo="$(escape_path_for_sed "${repo_root}")"
  escaped_home="$(escape_path_for_sed "${HOME}")"
  escaped_binary="$(escape_path_for_sed "${binary_path}")"
  escaped_gateway="$(escape_path_for_sed "${gateway_url}")"
  escaped_agent_id="$(escape_path_for_sed "${agent_id}")"
  escaped_agent_name="$(escape_path_for_sed "${agent_name}")"
  escaped_device_id="$(escape_path_for_sed "${device_id}")"
  escaped_secret="$(escape_path_for_sed "${shared_secret}")"
  escaped_update_manifest="$(escape_path_for_sed "${update_manifest_url}")"
  escaped_update_channel="$(escape_path_for_sed "${update_channel}")"
  escaped_update_interval="$(escape_path_for_sed "${update_interval_secs}")"
  escaped_update_status="$(escape_path_for_sed "${update_status_path}")"
  escaped_service_name="$(escape_path_for_sed "${service_name}")"
}

render_plist() {
  mkdir -p "${HOME}/Library/LaunchAgents" "${HOME}/Library/Logs/SpeechMesh"
  render_common_subst
  sed \
    -e "s|__REPO_ROOT__|${escaped_repo}|g" \
    -e "s|__HOME__|${escaped_home}|g" \
    -e "s|__BINARY_PATH__|${escaped_binary}|g" \
    -e "s|__GATEWAY_URL__|${escaped_gateway}|g" \
    -e "s|__AGENT_ID__|${escaped_agent_id}|g" \
    -e "s|__AGENT_NAME__|${escaped_agent_name}|g" \
    -e "s|__DEVICE_ID__|${escaped_device_id}|g" \
    -e "s|__SHARED_SECRET__|${escaped_secret}|g" \
    "${template_plist}" > "${plist_dst}"
  plutil -lint "${plist_dst}" >/dev/null
}

render_updater_plist() {
  mkdir -p "${HOME}/Library/LaunchAgents" "${HOME}/Library/Logs/SpeechMesh" "${HOME}/Library/Application Support/SpeechMesh"
  render_common_subst
  sed \
    -e "s|__REPO_ROOT__|${escaped_repo}|g" \
    -e "s|__HOME__|${escaped_home}|g" \
    -e "s|__BINARY_PATH__|${escaped_binary}|g" \
    -e "s|__UPDATE_MANIFEST_URL__|${escaped_update_manifest}|g" \
    -e "s|__UPDATE_CHANNEL__|${escaped_update_channel}|g" \
    -e "s|__UPDATE_INTERVAL_SECS__|${escaped_update_interval}|g" \
    -e "s|__UPDATE_STATUS_PATH__|${escaped_update_status}|g" \
    "${template_updater_plist}" > "${updater_plist_dst}"
  plutil -lint "${updater_plist_dst}" >/dev/null
}

render_unit() {
  if ! command -v systemctl >/dev/null 2>&1; then
    echo "systemctl is required for Linux device-agent service management" >&2
    exit 1
  fi
  mkdir -p "${HOME}/.config/systemd/user" "${HOME}/.local/state/speechmesh"
  render_common_subst
  sed \
    -e "s|__REPO_ROOT__|${escaped_repo}|g" \
    -e "s|__HOME__|${escaped_home}|g" \
    -e "s|__BINARY_PATH__|${escaped_binary}|g" \
    -e "s|__GATEWAY_URL__|${escaped_gateway}|g" \
    -e "s|__AGENT_ID__|${escaped_agent_id}|g" \
    -e "s|__AGENT_NAME__|${escaped_agent_name}|g" \
    -e "s|__DEVICE_ID__|${escaped_device_id}|g" \
    -e "s|__SHARED_SECRET__|${escaped_secret}|g" \
    "${template_unit}" > "${unit_dst}"
}

render_updater_unit() {
  render_common_subst
  sed \
    -e "s|__REPO_ROOT__|${escaped_repo}|g" \
    -e "s|__HOME__|${escaped_home}|g" \
    -e "s|__BINARY_PATH__|${escaped_binary}|g" \
    -e "s|__UPDATE_MANIFEST_URL__|${escaped_update_manifest}|g" \
    -e "s|__UPDATE_CHANNEL__|${escaped_update_channel}|g" \
    -e "s|__UPDATE_STATUS_PATH__|${escaped_update_status}|g" \
    -e "s|__SERVICE_NAME__|${escaped_service_name}|g" \
    "${template_updater_unit}" > "${updater_unit_dst}"
  sed \
    -e "s|__UPDATE_INTERVAL_SECS__|${escaped_update_interval}|g" \
    -e "s|__UPDATER_SERVICE_NAME__|$(escape_path_for_sed "${updater_service_name}")|g" \
    "${template_updater_timer}" > "${updater_timer_dst}"
}

build_binary() {
  if [[ "${skip_build}" == "true" ]]; then
    return
  fi
  cargo build -p speechmesh-app --release --bin speechmesh
}

install_binary() {
  if [[ "${skip_install_binary}" == "true" ]]; then
    return
  fi
  if [[ ! -x "${built_binary}" ]]; then
    echo "missing built binary: ${built_binary}" >&2
    exit 1
  fi
  mkdir -p "${binary_dir}"
  cp "${built_binary}" "${binary_path}"
  chmod +x "${binary_path}"
}

ensure_binary_exists() {
  if [[ ! -x "${binary_path}" ]]; then
    echo "missing installed binary: ${binary_path}" >&2
    exit 1
  fi
}

write_wrapper() {
  local target="$1"
  local mode="$2"
  if [[ "${mode}" == "agent" ]]; then
    cat > "${target}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${binary_path}" agent "\$@"
EOF
  else
    cat > "${target}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec "${binary_path}" "\$@"
EOF
  fi
  chmod +x "${target}"
}

apply_legacy_compat() {
  local legacy_agent="${binary_dir}/speechmesh-agent"
  local legacy_cli="${binary_dir}/speechmesh-cli"
  case "${legacy_compat}" in
    keep)
      ;;
    wrap)
      mkdir -p "${binary_dir}"
      write_wrapper "${legacy_agent}" "agent"
      write_wrapper "${legacy_cli}" "cli"
      ;;
    remove)
      rm -f "${legacy_agent}" "${legacy_cli}"
      ;;
    *)
      echo "invalid --legacy-compat value: ${legacy_compat}" >&2
      exit 1
      ;;
  esac
}

bootout_service() {
  launchctl bootout "gui/${uid}" "${plist_dst}" >/dev/null 2>&1 || true
}

bootout_updater_service() {
  launchctl bootout "gui/${uid}" "${updater_plist_dst}" >/dev/null 2>&1 || true
}

bootstrap_service() {
  launchctl bootstrap "gui/${uid}" "${plist_dst}"
  launchctl kickstart -k "gui/${uid}/${launchd_label}"
}

bootstrap_updater_service() {
  launchctl bootstrap "gui/${uid}" "${updater_plist_dst}"
  launchctl kickstart -k "gui/${uid}/${updater_launchd_label}" >/dev/null 2>&1 || true
}

status_service() {
  launchctl print "gui/${uid}/${launchd_label}" | sed -n '1,120p'
}

status_updater_service() {
  launchctl print "gui/${uid}/${updater_launchd_label}" | sed -n '1,120p'
}

linux_enable_service() {
  systemctl --user daemon-reload
  systemctl --user enable --now "${service_name}.service"
}

linux_enable_updater() {
  systemctl --user daemon-reload
  systemctl --user enable --now "${updater_service_name}.timer"
}

linux_stop_service() {
  systemctl --user disable --now "${service_name}.service" >/dev/null 2>&1 || true
}

linux_stop_updater() {
  systemctl --user disable --now "${updater_service_name}.timer" >/dev/null 2>&1 || true
  systemctl --user stop "${updater_service_name}.service" >/dev/null 2>&1 || true
}

linux_status_service() {
  systemctl --user --no-pager --full status "${service_name}.service" | sed -n '1,120p'
}

linux_status_updater() {
  systemctl --user --no-pager --full status "${updater_service_name}.timer" | sed -n '1,120p'
}

auto_update_enabled() {
  [[ "${disable_auto_update}" != "true" && -n "${update_manifest_url}" ]]
}

install_platform_service() {
  if [[ "${platform}" == "macos" ]]; then
    render_plist
    bootout_service
    bootstrap_service
    if auto_update_enabled; then
      render_updater_plist
      bootout_updater_service
      bootstrap_updater_service
    else
      bootout_updater_service
      rm -f "${updater_plist_dst}"
    fi
    status_service
    if auto_update_enabled; then
      echo "---"
      status_updater_service
    fi
    return
  fi
  render_unit
  linux_enable_service
  if auto_update_enabled; then
    render_updater_unit
    linux_enable_updater
  else
    linux_stop_updater
    rm -f "${updater_unit_dst}" "${updater_timer_dst}"
    systemctl --user daemon-reload
  fi
  linux_status_service
  if auto_update_enabled; then
    echo "---"
    linux_status_updater
  fi
}

stop_platform_service() {
  if [[ "${platform}" == "macos" ]]; then
    bootout_service
    bootout_updater_service
    return
  fi
  linux_stop_service
  linux_stop_updater
}

status_platform_service() {
  if [[ "${platform}" == "macos" ]]; then
    status_service
    if [[ -f "${updater_plist_dst}" ]]; then
      echo "---"
      status_updater_service
    fi
    return
  fi
  linux_status_service
  if [[ -f "${updater_timer_dst}" ]]; then
    echo "---"
    linux_status_updater
  fi
}

uninstall_platform_service() {
  if [[ "${platform}" == "macos" ]]; then
    bootout_service
    bootout_updater_service
    rm -f "${plist_dst}"
    rm -f "${updater_plist_dst}"
    return
  fi
  linux_stop_service
  linux_stop_updater
  rm -f "${unit_dst}" "${updater_unit_dst}" "${updater_timer_dst}"
  systemctl --user daemon-reload
}

install_service() {
  detect_platform
  build_binary
  install_binary
  ensure_binary_exists
  apply_legacy_compat
  install_platform_service
}

restart_service() {
  install_service
}

stop_service() {
  detect_platform
  stop_platform_service
}

status_service_action() {
  detect_platform
  status_platform_service
}

uninstall_service() {
  detect_platform
  uninstall_platform_service
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
    status_service_action
    ;;
  uninstall)
    uninstall_service
    ;;
esac
