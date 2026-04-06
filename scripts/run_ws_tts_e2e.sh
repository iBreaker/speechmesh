#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
  cat <<'EOF'
usage: run_ws_tts_e2e.sh <ws-url> <text> <output-wav> [provider-id]

example:
  scripts/run_ws_tts_e2e.sh \
    ws://127.0.0.1:8765/ws \
    "你好，这是 SpeechMesh 的 TTS 测试。" \
    /tmp/speechmesh-tts.wav \
    melo.tts
EOF
  exit 1
fi

ws_url="$1"
text="$2"
output_path="$3"
provider_id="${4:-melo.tts}"

node - "$ws_url" "$text" "$output_path" "$provider_id" <<'NODE'
const fs = require('fs');

const [, , url, text, outputPath, providerId] = process.argv;
const audioChunks = [];
let sessionId = null;
let sawDone = false;

const ws = new WebSocket(url);

function send(message) {
  ws.send(JSON.stringify(message));
}

ws.onopen = () => {
  send({
    type: 'hello',
    payload: {
      protocol_version: 'v1',
      client_name: 'speechmesh-ws-tts-e2e',
    },
  });
  send({
    type: 'tts.voices',
    request_id: 'req_voices',
    payload: {
      provider: {
        mode: 'provider',
        provider_id: providerId,
        required_capabilities: [],
        preferred_capabilities: [],
      },
      language: null,
    },
  });
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data.toString());
  switch (message.type) {
    case 'hello.ok':
      return;
    case 'tts.voices.result':
      send({
        type: 'tts.start',
        request_id: 'req_tts_start',
        payload: {
          provider: {
            mode: 'provider',
            provider_id: providerId,
            required_capabilities: [],
            preferred_capabilities: [],
          },
          input_kind: 'text',
          options: {
            language: null,
            voice: null,
            stream: true,
          },
        },
      });
      return;
    case 'session.started':
      sessionId = message.session_id;
      send({
        type: 'tts.input.append',
        session_id: sessionId,
        payload: {
          delta: text,
        },
      });
      send({
        type: 'tts.commit',
        session_id: sessionId,
        payload: {},
      });
      return;
    case 'tts.audio.delta':
      audioChunks.push(Buffer.from(message.payload.audio_base64, 'base64'));
      return;
    case 'tts.audio.done':
      sawDone = true;
      return;
    case 'session.ended':
      if (!sawDone) {
        console.error('session ended before tts.audio.done');
        process.exit(1);
      }
      fs.writeFileSync(outputPath, Buffer.concat(audioChunks));
      if (fs.statSync(outputPath).size === 0) {
        console.error('TTS output file is empty');
        process.exit(1);
      }
      ws.close();
      return;
    case 'error':
      console.error(JSON.stringify(message, null, 2));
      process.exit(1);
      return;
    default:
      return;
  }
};

ws.onerror = (error) => {
  console.error(error);
  process.exit(1);
};

ws.onclose = () => process.exit(0);
NODE

echo "wrote ${output_path}"
