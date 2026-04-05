#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  deploy_k8s.sh [--image-tag TAG] [--ingress-host HOST] [--agent-shared-secret SECRET] [--remote-host HOST] [--skip-build] [--skip-import]

Default behavior:
  1) Build linux/amd64 image locally
  2) Save and copy image tar to k3s-host
  3) Import image into k3s containerd
  4) Apply deploy/k8s/speechmesh.yaml with image/host/secret substitutions
  5) Wait for rollout in namespace apps
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

remote_host="k3s-host"
image_name="speechmeshd"
image_tag="$(date +%Y%m%d-%H%M%S)"
ingress_host="speechmesh.example.com"
agent_shared_secret="change-me"
namespace="apps"
skip_build="false"
skip_import="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --image-tag)
      image_tag="$2"
      shift 2
      ;;
    --ingress-host)
      ingress_host="$2"
      shift 2
      ;;
    --agent-shared-secret)
      agent_shared_secret="$2"
      shift 2
      ;;
    --remote-host)
      remote_host="$2"
      shift 2
      ;;
    --skip-build)
      skip_build="true"
      shift
      ;;
    --skip-import)
      skip_import="true"
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

image_ref="${image_name}:${image_tag}"
manifest_src="${repo_root}/deploy/k8s/speechmesh.yaml"
manifest_tmp="/tmp/speechmesh.${image_tag}.yaml"
image_tar="/tmp/${image_name}.${image_tag}.tar"
remote_tar="/tmp/${image_name}.${image_tag}.tar"

cleanup() {
  rm -f "${manifest_tmp}" "${image_tar}"
}
trap cleanup EXIT

if [[ "${skip_build}" != "true" ]]; then
  docker build --platform linux/amd64 -t "${image_ref}" "${repo_root}"
fi

if [[ "${skip_import}" != "true" ]]; then
  docker save "${image_ref}" -o "${image_tar}"
  scp "${image_tar}" "${remote_host}:${remote_tar}"
  ssh "${remote_host}" "sudo k3s ctr -n k8s.io images import '${remote_tar}' && rm -f '${remote_tar}'"
fi

sed \
  -e "s|image: speechmeshd:[^[:space:]]*|image: ${image_ref}|g" \
  -e "s|change-me|${agent_shared_secret}|g" \
  -e "s|speechmesh.example.com|${ingress_host}|g" \
  "${manifest_src}" > "${manifest_tmp}"

ssh "${remote_host}" "sudo k3s kubectl apply -f -" < "${manifest_tmp}"
ssh "${remote_host}" "sudo k3s kubectl -n ${namespace} rollout status deployment/speechmesh --timeout=180s"
ssh "${remote_host}" "sudo k3s kubectl -n ${namespace} get deployment/speechmesh service/speechmesh ingress/speechmesh-int -o wide"

echo "Deployed image: ${image_ref}"
echo "Ingress host: ${ingress_host}"
