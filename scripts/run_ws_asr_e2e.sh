#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  run_ws_asr_e2e.sh <ws-url> [expected-text]

Example:
  run_ws_asr_e2e.sh ws://127.0.0.1:8080/ws "speech mesh"
EOF
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

ws_url="$1"
expected_text="${2:-speech mesh}"
wav_path="${TMPDIR:-/tmp}/speechmesh_ws_asr_e2e.wav"
say_text="hello from speech mesh this is an end to end streaming test"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

"${repo_root}/scripts/prepare_asr_wav.sh" --say "${say_text}" "${wav_path}"

cargo run --manifest-path "${repo_root}/examples/ws-asr-e2e/Cargo.toml" -- \
  --url "${ws_url}" \
  --wav "${wav_path}" \
  --language "en-US" \
  --expected "${expected_text}"
