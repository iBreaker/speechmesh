#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  build_update_manifest.sh \
    --version VERSION \
    --channel CHANNEL \
    --release-prefix URL \
    --output PATH \
    --asset PLATFORM:ARCH:FILE [...]

Example:
  build_update_manifest.sh \
    --version 0.1.0 \
    --channel stable \
    --release-prefix https://github.com/example/speechmesh/releases/download/v0.1.0 \
    --output releases/stable.json \
    --asset linux:x86_64:dist/speechmesh-v0.1.0-linux-x86_64 \
    --asset macos:aarch64:dist/speechmesh-v0.1.0-macos-aarch64
USAGE
}

version=""
channel="stable"
release_prefix=""
output=""
declare -a assets=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      version="$2"
      shift 2
      ;;
    --channel)
      channel="$2"
      shift 2
      ;;
    --release-prefix)
      release_prefix="$2"
      shift 2
      ;;
    --output)
      output="$2"
      shift 2
      ;;
    --asset)
      assets+=("$2")
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

if [[ -z "${version}" || -z "${release_prefix}" || -z "${output}" || "${#assets[@]}" -eq 0 ]]; then
  echo "missing required arguments" >&2
  usage >&2
  exit 1
fi

mkdir -p "$(dirname "${output}")"

python3 - "$version" "$channel" "$release_prefix" "$output" "${assets[@]}" <<'PY'
import hashlib
import json
import os
import sys
from pathlib import Path

version = sys.argv[1]
channel = sys.argv[2]
release_prefix = sys.argv[3].rstrip("/")
output = Path(sys.argv[4])
assets = sys.argv[5:]

manifest_assets = []
for spec in assets:
    platform, arch, file_path = spec.split(":", 2)
    path = Path(file_path)
    if not path.is_file():
        raise SystemExit(f"asset file not found: {path}")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    manifest_assets.append(
        {
            "platform": platform,
            "arch": arch,
            "url": f"{release_prefix}/{path.name}",
            "sha256": digest,
            "binary_name": "speechmesh",
        }
    )

payload = {
    "schema": "speechmesh/update-manifest.v1",
    "default_channel": channel,
    "releases": [
        {
            "version": version,
            "channel": channel,
            "assets": manifest_assets,
        }
    ],
}

output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
PY
