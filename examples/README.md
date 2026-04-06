# Examples

This directory contains runnable clients and validation tooling for SpeechMesh.

## Available Examples

### `ws-asr-e2e/`

Rust end-to-end WebSocket ASR streaming client.

- built on the first-party Rust SDK
- reads a WAV file and streams binary audio frames
- waits for revision-based `asr.result` events
- asserts the final transcript when an expected phrase is provided

### `../sdks/go/examples/stream_asr/`

Go end-to-end ASR streaming client.

- built on the first-party Go SDK
- reads a WAV file and streams audio incrementally
- prints interim and final ASR revisions
- asserts the final transcript when an expected phrase is provided

### `../scripts/run_ws_tts_e2e.sh`

Protocol-level TTS validation helper for the public `/ws` contract.

- exercises `tts.voices`, `tts.start`, `tts.input.append`, and `tts.commit`
- collects streamed `tts.audio.delta` chunks into a WAV file
- verifies `tts.audio.done` and `session.ended`
- works against any SpeechMesh TTS provider exposed through the gateway

## Quick Runs

From the repository root:

```bash
scripts/run_ws_asr_e2e.sh ws://127.0.0.1:8765/ws "speech mesh"
scripts/run_ws_tts_e2e.sh ws://127.0.0.1:8765/ws "hello from speech mesh" /tmp/speechmesh-tts.wav melo.tts
scripts/run_go_sdk_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

See `../docs/testing.md` for the full validation matrix.
