# Testing

This document describes the validation matrix for SpeechMesh.

## Test Layers

### Rust workspace

```bash
cargo test
```

Covers shared contracts, gateway behavior, and the Rust SDK integration test.

### Rust SDK only

```bash
cargo test -p speechmesh-sdk
```

### Go SDK

```bash
cd sdks/go && go test ./...
```

## Local End-to-End Validation

### Mock gateway

Start a mock gateway:

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr
```

Then run the Rust SDK E2E client:

```bash
scripts/run_ws_asr_e2e.sh ws://127.0.0.1:8765/ws "speech mesh"
```

Or run the Go SDK example directly with your own WAV file:

```bash
cd sdks/go
go run ./examples/stream_asr \
  --url ws://127.0.0.1:8765/ws \
  --wav /path/to/audio.wav \
  --expected "speech mesh"
```

## Live Split-Deployment Validation

Once the Linux gateway and macOS agent are up, validate the real route:

```bash
./scripts/run_ws_asr_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
./scripts/run_go_sdk_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

Expected behavior:

- the client completes the WebSocket handshake
- `session.started` reports the Apple-backed provider
- multiple interim `asr.result` revisions stream back
- the final transcript contains the expected phrase
- the command exits with success

## Audio Fixtures

The helper script `scripts/prepare_asr_wav.sh` normalizes audio into the format currently used by the ASR examples:

- mono
- 16 kHz
- PCM S16LE WAV

Examples:

```bash
scripts/prepare_asr_wav.sh --from ./input-any-format.wav /tmp/speechmesh.wav
scripts/prepare_asr_wav.sh --say "hello from speech mesh" /tmp/speechmesh.wav
```

Note: `--say` depends on the macOS `say` command.

## Troubleshooting

### No transcript arrives

Check:

- the gateway is reachable on `/ws`
- a provider is discoverable through `discover`
- the macOS agent is connected if you are using `agent` mode
- your input audio matches the declared `input_format`

### Agent-backed sessions fail

Check:

- the gateway and agent share the same secret
- the LaunchAgent is running on macOS
- the Apple bridge binary is present and executable
- ingress allows long-lived WebSocket connections

### Transcript text changes mid-stream

That is expected. ASR revisions can rewrite earlier text. Clients must render the latest `payload.text`.
