# SpeechMesh

SpeechMesh is a speech runtime for building local and remote speech systems behind a single, transport-neutral architecture.

It starts with two capability domains:

- ASR: speech-to-text
- TTS: text-to-speech

The current production path is WebSocket-first:

- `speechmeshd` is the Rust gateway/runtime daemon
- `/ws` is the client-facing WebSocket endpoint
- `/agent` is the agent-facing WebSocket endpoint for device speaker agents and provider agents (including Apple-backed providers)
- Apple Speech stays on macOS while the heavier gateway path runs in Linux or Kubernetes
- local or remote TTS engines can sit behind the same gateway contract
- first-party Rust and Go SDKs provide a stable client entry point for remote devices

The gateway can now also expose multiple explicitly installed ASR providers behind the same `/ws` endpoint.

## Status

SpeechMesh is pre-1.0 and still evolving, but the current ASR and initial TTS paths are already usable for real deployments.

Today the repository includes:

- streaming ASR over WebSocket
- WebSocket TTS sessions with append/commit audio streaming semantics
- a split deployment model for Linux gateway + macOS Apple Speech execution
- explicit provider catalogs and installed-provider state for ASR routing
- a first local TTS provider integration through MeloTTS
- Rust and Go client SDKs
- first-party Rust and Go SDK helpers for both ASR and TTS session flows
- local mock bridge mode for development and protocol testing
- Kubernetes and macOS service assets for the current production shape

## Why SpeechMesh

Speech stacks usually get trapped in one of two shapes:

- provider-specific SDK wrappers that are hard to compose
- product-specific daemons that are hard to extend

SpeechMesh aims for a cleaner boundary:

- capability-first design instead of vendor-first design
- shared transport contracts instead of provider-specific wire formats
- explicit provider capabilities instead of hidden behavior
- support for both local execution and remote routing

## Architecture At A Glance

```text
+-------------------+        +-------------------------+
| Go / Rust clients | -----> | speechmeshd gateway     |
| any device        |  /ws   | Linux / Kubernetes      |
+-------------------+        +------------+------------+
                                          |
                                          | /agent
                                          v
                               +----------+-----------+
                               | apple_agent          |
                               | macOS lightweight    |
                               +----------+-----------+
                                          |
                                          | local process
                                          v
                               +----------+-----------+
                               | apple-asr-bridge     |
                               | Apple Speech.framework|
                               +----------------------+
```

Other runtime shapes are also supported for development and testing:

- `mock` bridge mode for synthetic transcripts
- `stdio` bridge mode for subprocess-backed providers
- `tcp` bridge mode for remote bridge processes
- `agent` bridge mode for the current Linux gateway -> macOS agent production split

## Quick Start

### 1. Run the test suite

```bash
cargo test
cd sdks/go && go test ./...
```

### 2. Start a local mock gateway

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr
```

### 3. Drive a local end-to-end ASR session

```bash
scripts/run_ws_asr_e2e.sh ws://127.0.0.1:8765/ws "speech mesh"
```

`run_ws_asr_e2e.sh` and `run_go_sdk_e2e.sh` synthesize test audio through `say`, so they are macOS-oriented helpers. On Linux, generate a compatible WAV file manually and run the example binaries directly.

### 3.5 Drive a local end-to-end TTS session through MeloTTS

If your local MeloTTS server is already running on `http://127.0.0.1:7797`, start the gateway with both ASR and TTS enabled:

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr \
  --tts-bridge-mode melo-http \
  --tts-provider-id melo.tts \
  --tts-provider-name MeloTTS \
  --tts-melo-base-url http://127.0.0.1:7797
```

Then validate TTS over WebSocket:

```bash
scripts/run_ws_tts_e2e.sh \
  ws://127.0.0.1:8765/ws \
  "你好，这是 SpeechMesh 的 MeloTTS 集成测试。" \
  /tmp/speechmesh-tts.wav \
  melo.tts
```

### 4. Install providers explicitly, then start a multi-provider gateway

Supported providers live in a catalog. A provider only becomes discoverable after you install it into gateway state.

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers install apple.asr \
  --catalog deploy/providers.catalog.example.json \
  --state /tmp/speechmesh.providers.json
```

Then point the gateway at the installed-provider state:

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-providers-state /tmp/speechmesh.providers.json
```

The install boundary now works like this:

- if a provider is not present in the state file, it is treated as not installed
- only installed and enabled providers are returned by `discover`
- supported providers can exist in the catalog without being exposed on this gateway
- routed providers can still fail at session start if their bridge or agent is temporarily unavailable

## Production Split Deployment

Apple `Speech.framework` cannot run inside a Linux container. The supported production topology is:

- `speechmeshd` in Linux or Kubernetes
- `apple_agent` on a trusted macOS host
- `apple-asr-bridge` launched locally by the agent on that same macOS host

The repository already includes:

- container assets: `Dockerfile`, `.dockerignore`
- Kubernetes manifest: `deploy/k8s/speechmesh.yaml`
- Linux deployment helper: `scripts/deploy_k8s.sh`
- macOS LaunchAgent asset: `deploy/macos/io.speechmesh.apple-agent.plist`
- macOS installer helper: `scripts/install_apple_agent_service.sh`
- device-agent LaunchAgent asset: `deploy/macos/io.speechmesh.device-agent.plist`
- Linux systemd user unit template: `deploy/linux/speechmesh-device-agent.service`
- cross-platform device-agent installer helper: `scripts/install_device_agent_service.sh`

Typical flow:

```bash
./scripts/deploy_k8s.sh --image-tag 20260405-1
./scripts/install_apple_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id apple-agent-1 \
  --agent-name "Apple ASR Agent" \
  --shared-secret "change-me"
./scripts/install_device_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id mac01-speaker-agent \
  --agent-name "Mac 01 Speaker Agent" \
  --device-id mac01 \
  --shared-secret "change-me"
# on Linux host (systemd --user):
./scripts/install_device_agent_service.sh install \
  --platform linux \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id linux01-speaker-agent \
  --agent-name "Linux 01 Speaker Agent" \
  --device-id linux01 \
  --shared-secret "change-me"
./scripts/run_ws_asr_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

## WebSocket Contract Highlights

SpeechMesh v1 is a WebSocket-first streaming protocol.

- JSON text frames carry control messages such as `hello`, `discover`, `asr.start`, `asr.commit`, and `session.stop`
- TTS uses JSON text frames end-to-end, including `tts.start`, `tts.input.append`, `tts.commit`, `tts.audio.delta`, and `tts.audio.done`
- binary frames carry raw audio bytes for the active ASR session
- each connection supports at most one active session at a time
- `asr.result` events are revision-based snapshots, not append-only token streams
- clients should treat `payload.text` as the source of truth and use `payload.delta` only as an optimization

## SDKs

First-party client SDKs live in:

- `sdks/rust` - async Rust SDK
- `sdks/go` - Go SDK

Examples:

- Rust SDK E2E client: `examples/ws-asr-e2e`
- Go SDK E2E client: `sdks/go/examples/stream_asr`
- TTS protocol smoke test: `scripts/run_ws_tts_e2e.sh`

## Repository Map

```text
speechmesh/
  app/                  unified client binary entrypoint (`speechmesh`)
  asr/                  ASR contracts and provider-facing types
  bridges/apple-asr/    internal macOS Apple Speech bridge
  core/                 shared runtime concepts
  docs/                 architecture, protocol, deployment, testing docs
  examples/             runnable clients and validation tools
  sdks/go/              first-party Go client SDK
  sdks/rust/            first-party Rust client SDK
  speechmeshd/          WebSocket gateway daemon (`speechmeshd`)
  transport/            shared transport contract types
  tts/                  TTS contracts and provider-facing types
```

## Documentation

Start here:

- `docs/README.md`
- `docs/architecture.md`
- `docs/websocket-protocol.md`
- `docs/deployment.md`
- `docs/providers.md`
- `docs/tts-websocket-design.md`
- `docs/sdk-guide.md`
- `docs/testing.md`
- `docs/compatibility.md`
- `docs/roadmap.md`
- `docs/tts-landscape.md`

Component-level references:

- `speechmeshd/README.md`
- `bridges/apple-asr/README.md`
- `sdks/README.md`
- `examples/README.md`

## Development Notes

- keep shared transport contracts generic and provider-neutral
- add providers under their capability domain rather than under a global vendor layer
- keep `payload.text` authoritative for streamed ASR rendering
- distinguish the public WebSocket contract from internal bridge protocols
- update docs whenever protocol or deployment behavior changes

See `CONTRIBUTING.md` for the development workflow and `SECURITY.md` for reporting guidance.

## License

MIT. See `LICENSE`.
