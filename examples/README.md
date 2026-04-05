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

## Quick Runs

From the repository root:

```bash
scripts/run_ws_asr_e2e.sh ws://127.0.0.1:8765/ws "speech mesh"
scripts/run_go_sdk_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

See `../docs/testing.md` for the full validation matrix.
