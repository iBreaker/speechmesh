# speechmeshd

`speechmeshd` is the current SpeechMesh runtime gateway.

It exposes the public WebSocket API, coordinates one active speech session per client connection, and routes both ASR and TTS work to bridge backends.

## Binaries

- `speechmeshd` - client-facing WebSocket gateway
- `apple_agent` - macOS connector that registers an Apple-backed provider with the gateway
- `bridge_tcpd` - TCP adapter for line-based bridge processes

## Endpoints

- `/ws` - client traffic
- `/agent` - gateway-to-agent traffic used in `agent` mode

## Runtime Layout

The gateway keeps ASR and TTS runtime code symmetric:

- `src/asr_bridge.rs` - ASR bridge traits and bridge implementations
- `src/tts_bridge.rs` - TTS bridge traits and bridge implementations
- `src/bridge_support.rs` - shared bridge error types
- `src/server.rs` - unified `/ws` protocol handling for both domains

## Supported ASR Bridge Modes

| Mode | Purpose | Typical Use |
| --- | --- | --- |
| `mock` | synthetic transcript generation | local development and protocol tests |
| `stdio` | local subprocess bridge | local provider integration |
| `tcp` | remote line-based bridge | trusted-network bridge host |
| `agent` | registered remote agent | Linux gateway + macOS Apple Speech |

## Supported TTS Bridge Modes

| Mode | Purpose | Typical Use |
| --- | --- | --- |
| `disabled` | no TTS provider is exposed | ASR-only deployments |
| `mock` | synthetic bytes for protocol tests | transport and client validation |
| `melo_http` | local or remote MeloTTS HTTP service | first real TTS provider path |

## `speechmeshd` Usage

```bash
cargo run -p speechmeshd --bin speechmeshd -- --help
```

### Provider lifecycle commands

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers list \
  --catalog deploy/providers.catalog.example.json \
  --state /tmp/speechmesh.providers.json

cargo run -p speechmeshd --bin speechmeshd -- providers install apple.asr \
  --catalog deploy/providers.catalog.example.json \
  --state /tmp/speechmesh.providers.json
```

### Local mock mode

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr
```

### Remote agent mode

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 0.0.0.0:8765 \
  --server-name speechmesh-gateway \
  --asr-bridge-mode agent \
  --asr-provider-id apple.asr \
  --agent-shared-secret change-me
```

### Installed-provider state mode

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 0.0.0.0:8765 \
  --server-name speechmesh-gateway \
  --asr-providers-state /tmp/speechmesh.providers.json
```

### MeloTTS mode

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

## `apple_agent` Usage

```bash
cargo run -p speechmeshd --bin apple_agent -- \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id apple-agent-1 \
  --agent-name "Apple ASR Agent" \
  --provider-id apple.asr \
  --shared-secret change-me \
  --bridge-command /path/to/apple-asr-bridge
```

## `bridge_tcpd` Usage

```bash
cargo run -p speechmeshd --bin bridge_tcpd -- \
  --listen 0.0.0.0:9654 \
  --bridge-command /path/to/bridge-binary
```

## Result Model

`speechmeshd` emits revision-based `asr.result` events and chunked `tts.audio.delta` events.

Important client rules:

- `payload.text` is the current truth
- `payload.delta` is best-effort only
- final completion is `is_final=true` and `speech_final=true`
- TTS completion is `tts.audio.done`, followed by `session.ended`

## Protocol Surface Separation

`speechmeshd` is responsible for the public WebSocket protocol. Internal bridge implementations may use different message shapes such as `asr.partial` or `asr.final`, but those are not the public client contract.

## Deployment Notes

For the current Apple ASR production path:

- install `apple.asr` into provider state and run `speechmeshd` against `--asr-providers-state`
- run `apple_agent` on macOS as a LaunchAgent
- keep the Apple bridge local to that macOS host

See `docs/deployment.md` for the full deployment workflow.
