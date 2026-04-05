# Contributing

Thanks for contributing to SpeechMesh.

## Project Priorities

SpeechMesh is opinionated about architecture. When contributing, optimize for:

- capability-first boundaries over vendor-first shortcuts
- stable shared contracts over provider-specific wire formats
- explicit runtime and provider capabilities over hidden assumptions
- small, reviewable changes with matching documentation updates

## Development Setup

### Requirements

- Rust toolchain with `cargo`
- Go toolchain for the Go SDK
- `ffmpeg` for audio normalization helpers
- macOS only: `say` for the built-in E2E helper scripts

### Bootstrap

```bash
cargo test
cd sdks/go && go test ./...
```

## Repository Workflow

1. make a focused change
2. run the relevant tests
3. update docs if behavior, protocol, deployment, or SDK surfaces changed
4. keep provider-specific logic inside its capability domain
5. avoid breaking the shared transport contract without documenting the migration impact

## Coding Guidelines

### Rust

- keep transport contracts provider-neutral
- prefer small structs and enums with explicit names
- surface protocol changes in tests and docs together

### Go SDK

- keep the SDK close to the public wire contract
- preserve streaming semantics instead of over-hiding them
- keep examples runnable and realistic

### Documentation

Docs are part of the product surface.

Update the relevant files whenever you change:

- protocol behavior
- deployment topology
- CLI flags or scripts
- SDK method behavior
- example flows

## Validation Checklist

Run the commands that match your change set:

```bash
cargo test
cargo test -p speechmesh-sdk
cd sdks/go && go test ./...
```

For deployment or live-routing changes, also run the live E2E checks documented in `docs/testing.md`.

## Architectural Guardrails

- providers belong under `asr/` or `tts/`, not under a global vendor directory
- transports carry speech work but should not own speech logic
- clients must treat `payload.text` as authoritative for ASR rendering
- one active session per connection is the current contract unless explicitly changed

## Pull Request Scope

A good contribution typically includes:

- the code change
- tests or validation updates
- documentation updates
- a short explanation of user-visible or operator-visible impact
