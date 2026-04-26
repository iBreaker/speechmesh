#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  install_termux_device_agent_service.sh [install|restart|stop|status|uninstall] [options]

Options:
  --repo-root PATH        speechmesh repository root, default current directory
  --skip-build            Skip cargo build
  --skip-install-binary   Skip copying binary into --binary-path
  --binary-path PATH      Installed speechmesh path, default $PREFIX/bin/speechmesh
  --gateway-url URL       Gateway /agent URL, default wss://speechmesh.example.com/agent
  --agent-id ID           Agent id, default android01-speaker-agent
  --agent-name NAME       Agent name, default "Android 01 Speaker Agent"
  --device-id ID          Device id, default android01
  --shared-secret SECRET  Shared secret used by gateway and agent
  --service-name NAME     Termux service name, default speechmesh-device-agent
  --playback-cmd CMD      Override local playback command via SPEECHMESH_PLAYBACK_CMD
  --enable-boot           Install a Termux:Boot helper script
USAGE
}

action="install"
repo_root="$(pwd)"
skip_build="false"
skip_install_binary="false"
binary_path="${PREFIX:-/data/data/com.termux/files/usr}/bin/speechmesh"
gateway_url="wss://speechmesh.example.com/agent"
agent_id="android01-speaker-agent"
agent_name="Android 01 Speaker Agent"
device_id="android01"
shared_secret="change-me"
service_name="speechmesh-device-agent"
playback_cmd=""
enable_boot="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    install|restart|stop|status|uninstall)
      action="$1"
      shift
      ;;
    --repo-root)
      repo_root="$2"
      shift 2
      ;;
    --skip-build)
      skip_build="true"
      shift
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
    --service-name)
      service_name="$2"
      shift 2
      ;;
    --playback-cmd)
      playback_cmd="$2"
      shift 2
      ;;
    --enable-boot)
      enable_boot="true"
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

if [[ ! -d "${repo_root}" ]]; then
  echo "repo root not found: ${repo_root}" >&2
  exit 1
fi

if [[ -z "${PREFIX:-}" || "${PREFIX}" != *"/com.termux/files/usr" ]]; then
  echo "this installer must run inside Termux (PREFIX=${PREFIX:-unset})" >&2
  exit 1
fi

if ! command -v sv >/dev/null 2>&1; then
  cat >&2 <<'ERR'
termux-services is required.
Run:
  pkg install termux-services
  source $PREFIX/etc/profile.d/start-services.sh
ERR
  exit 1
fi

service_root="${HOME}/.termux/service/${service_name}"
run_script="${service_root}/run"
log_run_script="${service_root}/log/run"
boot_script="${HOME}/.termux/boot/${service_name}.sh"
built_binary="${repo_root}/target/release/speechmesh"
binary_dir="$(dirname "${binary_path}")"

q() {
  printf '%q' "$1"
}

build_binary() {
  if [[ "${skip_build}" == "true" ]]; then
    return
  fi
  (cd "${repo_root}" && cargo build -p speechmesh-app --release --bin speechmesh)
}

install_binary() {
  if [[ "${skip_install_binary}" == "true" ]]; then
    return
  fi
  if [[ ! -f "${built_binary}" ]]; then
    echo "built binary not found: ${built_binary}" >&2
    echo "build first or pass --skip-install-binary with an existing --binary-path" >&2
    exit 1
  fi
  mkdir -p "${binary_dir}"
  cp "${built_binary}" "${binary_path}"
  chmod 0755 "${binary_path}"
}

render_service() {
  mkdir -p "${service_root}" "${service_root}/log"

  local qb qgw qaid qaname qdid qsecret qplay
  qb="$(q "${binary_path}")"
  qgw="$(q "${gateway_url}")"
  qaid="$(q "${agent_id}")"
  qaname="$(q "${agent_name}")"
  qdid="$(q "${device_id}")"
  qsecret="$(q "${shared_secret}")"
  qplay="$(q "${playback_cmd}")"

  cat >"${run_script}" <<EOF
#!/data/data/com.termux/files/usr/bin/bash
set -euo pipefail
export RUST_LOG=\${RUST_LOG:-info}
if [[ -n ${qplay} ]]; then
  export SPEECHMESH_PLAYBACK_CMD=${qplay}
fi
exec ${qb} agent run \\
  --gateway-url ${qgw} \\
  --agent-id ${qaid} \\
  --agent-name ${qaname} \\
  --device-id ${qdid} \\
  --provider-id device.speaker \\
  --shared-secret ${qsecret} \\
  --capability speaker \\
  --reconnect-delay-secs 2
EOF

  cat >"${log_run_script}" <<'EOF'
#!/data/data/com.termux/files/usr/bin/bash
exec svlogger -tt ./main
EOF

  chmod 0755 "${run_script}" "${log_run_script}"
}

install_boot_script() {
  mkdir -p "${HOME}/.termux/boot"
  cat >"${boot_script}" <<EOF
#!/data/data/com.termux/files/usr/bin/bash
source "\$PREFIX/etc/profile.d/start-services.sh"
termux-wake-lock || true
sv up ${service_name} || true
EOF
  chmod 0755 "${boot_script}"
}

case "${action}" in
  install)
    build_binary
    install_binary
    render_service
    if [[ "${enable_boot}" == "true" ]]; then
      install_boot_script
    fi
    sv up "${service_name}" || true
    echo "installed Termux service ${service_name}"
    echo "status:"
    sv status "${service_name}" || true
    ;;
  restart)
    sv restart "${service_name}"
    sv status "${service_name}"
    ;;
  stop)
    sv down "${service_name}"
    sv status "${service_name}" || true
    ;;
  status)
    sv status "${service_name}"
    ;;
  uninstall)
    sv down "${service_name}" || true
    rm -rf "${service_root}"
    rm -f "${boot_script}"
    echo "removed service ${service_name}"
    ;;
  *)
    echo "unsupported action: ${action}" >&2
    exit 1
    ;;
esac
