#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  run_go_sdk_e2e.sh <ws-url> [expected-text]

Example:
  run_go_sdk_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

ws_url="$1"
expected_text="${2:-speech mesh}"
wav_path="${TMPDIR:-/tmp}/speechmesh_go_sdk_e2e.wav"
say_text="hello from speech mesh this is a go sdk streaming test"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

"${repo_root}/scripts/prepare_asr_wav.sh" --say "${say_text}" "${wav_path}"

(
  cd "${repo_root}/sdks/go"
  go run ./examples/stream_asr \
    --url "${ws_url}" \
    --wav "${wav_path}" \
    --language "en-US" \
    --expected "${expected_text}"
)
