#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  prepare_asr_wav.sh --from <input-audio-file> <output.wav>
  prepare_asr_wav.sh --say <text> <output.wav>

Description:
  Normalize audio to SpeechMesh ASR test format:
  - mono
  - 16000 Hz
  - PCM S16LE WAV
EOF
}

if [[ $# -lt 3 ]]; then
  usage
  exit 1
fi

mode="$1"
value="$2"
output="$3"

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "ffmpeg is required but not found" >&2
  exit 1
fi

tmp_input=""
cleanup() {
  if [[ -n "${tmp_input}" && -f "${tmp_input}" ]]; then
    rm -f "${tmp_input}"
  fi
}
trap cleanup EXIT

case "${mode}" in
  --from)
    input="${value}"
    if [[ ! -f "${input}" ]]; then
      echo "input file not found: ${input}" >&2
      exit 1
    fi
    ffmpeg -y -i "${input}" -ac 1 -ar 16000 -c:a pcm_s16le "${output}" >/dev/null 2>&1
    ;;
  --say)
    if ! command -v say >/dev/null 2>&1; then
      echo "say command not found (macOS required for --say mode)" >&2
      exit 1
    fi
    # BSD `mktemp` on macOS treats suffix templates differently, so use `-t`
    # and let `say` write AIFF data to the unique temporary file path.
    tmp_input="$(mktemp -t speechmesh-say)"
    say -v Samantha -o "${tmp_input}" "${value}"
    ffmpeg -y -i "${tmp_input}" -ac 1 -ar 16000 -c:a pcm_s16le "${output}" >/dev/null 2>&1
    ;;
  *)
    usage
    exit 1
    ;;
esac

echo "prepared wav: ${output}"
