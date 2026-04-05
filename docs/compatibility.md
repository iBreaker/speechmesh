# Compatibility

SpeechMesh is currently pre-1.0.

Until tagged releases exist, the repository `main` branch is the source of truth and compatibility should be treated as commit-based.

## Current Compatibility Matrix

| Component | Public contract | Current expectation |
| --- | --- | --- |
| `speechmeshd` | WebSocket protocol `v1` | source of truth |
| `sdks/rust` | WebSocket protocol `v1` | compatible with current `speechmeshd` |
| `sdks/go` | WebSocket protocol `v1` | compatible with current `speechmeshd` |
| `examples/ws-asr-e2e` | Rust SDK + protocol `v1` | validation client |
| `sdks/go/examples/stream_asr` | Go SDK + protocol `v1` | validation client |

## Compatibility Rules

- clients must send `hello` with `protocol_version="v1"`
- SDKs should preserve `asr.result` revision semantics exactly
- one active session per connection is part of the current contract
- `payload.text` remains authoritative even when `payload.delta` changes shape

## Internal Versus Public Protocols

SpeechMesh exposes more than one protocol surface:

- public client API: WebSocket protocol on `/ws`
- internal agent protocol: WebSocket protocol on `/agent`
- internal bridge protocol: line-based JSON between the agent or gateway and provider bridge processes

Only the `/ws` public WebSocket contract should be treated as the stable third-party integration surface.

## Upgrade Guidance

Before upgrading a deployed system:

1. verify the target gateway still speaks protocol `v1`
2. run `cargo test`
3. run `cargo test -p speechmesh-sdk`
4. run `cd sdks/go && go test ./...`
5. rerun the live E2E validation commands from `docs/testing.md`

## Future Release Policy

Once tagged releases exist, this document should expand into a versioned compatibility table covering:

- gateway version
- Rust SDK version
- Go SDK version
- bridge version
- protocol version
