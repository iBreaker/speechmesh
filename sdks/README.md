# SDKs

SpeechMesh currently ships first-party client SDKs for the public WebSocket gateway.

## Available SDKs

| Language | Path | Status | Example |
| --- | --- | --- | --- |
| Rust | `sdks/rust` | usable | `examples/ws-asr-e2e` |
| Go | `sdks/go` | usable | `sdks/go/examples/stream_asr` |

## Design Intent

The SDKs are client-side only.

They do not embed Apple Speech locally and they do not bypass the SpeechMesh gateway. Instead:

- clients connect to the shared WebSocket endpoint
- the gateway handles provider discovery and routing
- Apple Speech execution still happens on the configured macOS host through the agent path

## Shared Behavior

All SDKs preserve the same runtime contract:

- automatic `hello` handshake on connect
- optional provider discovery
- one active session per connection
- binary audio streaming for ASR
- revision-based `asr.result` events
- explicit `commit`, `stop`, and `close` behavior

See `../docs/sdk-guide.md` for the cross-language guide.
