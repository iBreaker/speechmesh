#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  test_melotts_ws_bridge.sh [ws-url]

Example:
  test_melotts_ws_bridge.sh ws://127.0.0.1:8797/ws/tts
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

ws_url="${1:-ws://127.0.0.1:8797/ws/tts}"
text="${2:-MeloTTS bridge websocket smoke test.}"

node - <<'NODE' "${ws_url}" "${text}"
const wsUrl = process.argv[2];
const text = process.argv[3];
const ws = new WebSocket(wsUrl);

let sessionId = null;
let audioBytes = 0;
let chunkCount = 0;

ws.onopen = () => {
  ws.send(
    JSON.stringify({
      type: "hello",
      request_id: "req_hello",
      payload: { protocol_version: "v1", client_name: "speechmesh-melo-smoke" },
    }),
  );
  ws.send(
    JSON.stringify({
      type: "tts.start",
      request_id: "req_start",
      payload: {
        input: { type: "text", text },
        options: { rate: 1.0, chunk_bytes: 8192 },
      },
    }),
  );
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data.toString());
  if (msg.type === "session.started") {
    sessionId = msg.session_id;
    ws.send(
      JSON.stringify({
        type: "tts.commit",
        request_id: "req_commit",
        payload: { session_id: sessionId },
      }),
    );
    return;
  }
  if (msg.type === "tts.audio.chunk") {
    chunkCount += 1;
    const b = Buffer.from(msg.payload.data_base64, "base64");
    audioBytes += b.length;
    return;
  }
  if (msg.type === "session.ended") {
    if (!sessionId || chunkCount < 1 || audioBytes < 256) {
      console.error("unexpected test result", { sessionId, chunkCount, audioBytes, msg });
      process.exit(1);
    }
    console.log(JSON.stringify({ ok: true, sessionId, chunkCount, audioBytes }, null, 2));
    ws.close();
    return;
  }
  if (msg.type === "error") {
    console.error("bridge returned error", msg);
    process.exit(1);
  }
};

ws.onerror = (error) => {
  console.error("websocket error", error);
  process.exit(1);
};

ws.onclose = () => process.exit(0);
NODE
