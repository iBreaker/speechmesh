# SpeechMesh

SpeechMesh is a speech runtime for building local and remote speech systems behind a single, transport-neutral architecture.

It starts with two capability domains:

- ASR: speech-to-text
- TTS: text-to-speech

The current production path is ASR-first and WebSocket-first:

- `speechmeshd` is the Rust gateway/runtime daemon
- `/ws` is the client-facing WebSocket endpoint
- `/agent` is the macOS agent endpoint for Apple-backed providers
- Apple Speech stays on macOS while the heavier gateway path runs in Linux or Kubernetes
- first-party Rust and Go SDKs provide a stable client entry point for remote devices

## Status

SpeechMesh is pre-1.0 and still evolving, but the current ASR path is already usable for real deployments.

Today the repository includes:

- streaming ASR over WebSocket
- a split deployment model for Linux gateway + macOS Apple Speech execution
- Rust and Go client SDKs
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

Typical flow:

```bash
./scripts/deploy_k8s.sh --image-tag 20260405-1
./scripts/install_apple_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id apple-agent-1 \
  --agent-name "Apple ASR Agent" \
  --shared-secret "change-me"
./scripts/run_ws_asr_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

## WebSocket Contract Highlights

SpeechMesh v1 is a WebSocket-first streaming protocol.

- JSON text frames carry control messages such as `hello`, `discover`, `asr.start`, `asr.commit`, and `session.stop`
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

## Repository Map

```text
speechmesh/
  asr/                  ASR contracts and provider-facing types
  bridges/apple-asr/    internal macOS Apple Speech bridge
  core/                 shared runtime concepts
  docs/                 architecture, protocol, deployment, testing docs
  examples/             runnable clients and validation tools
  sdks/go/              first-party Go client SDK
  sdks/rust/            first-party Rust client SDK
  speechmeshd/          WebSocket gateway and agent binaries
  transport/            shared transport contract types
  tts/                  TTS contracts and provider-facing types
```

## Documentation

Start here:

- `docs/README.md`
- `docs/architecture.md`
- `docs/websocket-protocol.md`
- `docs/deployment.md`
- `docs/sdk-guide.md`
- `docs/testing.md`
- `docs/compatibility.md`
- `docs/roadmap.md`

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
