# Architecture

## Overview

SpeechMesh is a capability-first speech runtime.

Instead of hard-coding one vendor SDK into one product surface, SpeechMesh separates the system into four stable layers:

- domain contracts: ASR and TTS request and event models
- core runtime: capabilities, providers, sessions, errors, and lifecycle
- transport: shared wire-level messages and routing behavior
- execution backends: local providers, remote bridges, and platform-specific agents

## Design Goals

- keep ASR and TTS modeled as separate capability domains
- keep transports reusable across providers and runtime shapes
- support both local and remote execution paths
- preserve provider-specific capability differences without polluting the shared contract
- make split deployments first-class instead of an afterthought

## Current Implementation Shape

Today the active production topology is:

- `speechmeshd` as the WebSocket gateway and session coordinator
- `apple_agent` as the lightweight macOS-side connector
- `apple-asr-bridge` as the platform-specific Apple Speech execution process
- first-party Go and Rust SDKs as the canonical client surface

This lets the heavy, scalable control plane live in Linux or Kubernetes while keeping Apple-native recognition on a macOS host.

## Component Model

### `core`

Shared concepts that do not belong to one capability domain:

- session identifiers
- provider descriptors
- capability flags
- runtime modes
- audio format descriptors
- shared error structures

### `asr`

Speech-to-text contracts and provider-facing types:

- streaming request model
- transcript structures
- partial and final event shapes
- provider selection and recognition options

### `tts`

Text-to-speech contracts and provider-facing types. This exists as a domain boundary today, even though the current shipped transport path is ASR-focused.

### `transport`

Transport-neutral shared protocol types:

- client messages
- server messages
- request and response payloads
- event payloads

The transport layer carries speech work; it does not own speech logic.

### `speechmeshd`

Runtime gateway binaries:

- `speechmeshd` - WebSocket gateway
- `apple_agent` - agent that connects macOS execution to the gateway
- `bridge_tcpd` - TCP adapter for line-based external bridges

### `sdks`

Client-facing integrations that hide protocol boilerplate while preserving the streaming model.

## Runtime Shapes

SpeechMesh intentionally supports multiple runtime shapes.

### `mock`

Synthetic ASR results for local development, protocol tests, and SDK verification.

### `stdio`

Subprocess-backed bridge mode where `speechmeshd` launches a local provider bridge and exchanges line-delimited JSON.

### `tcp`

Remote bridge mode where `speechmeshd` talks to a bridge process over TCP using the same line-oriented bridge protocol.

### `agent`

Remote agent mode where `speechmeshd` exposes `/agent`, a macOS host connects in, and the gateway routes ASR sessions through that registered agent.

This is the primary deployment model for Apple Speech today.

## Session Lifecycle

A client session follows this flow:

1. open a WebSocket connection to `/ws`
2. send `hello`
3. optionally send `discover`
4. send `asr.start`
5. stream binary audio frames
6. send `asr.commit`
7. receive zero or more `asr.result` revisions
8. stop when `is_final=true` and `speech_final=true`
9. receive `session.ended`

In `agent` mode, the internal flow continues:

1. gateway selects a registered agent for the requested provider
2. gateway sends `session.start` to the agent
3. agent launches or reuses the local Apple bridge
4. agent forwards audio chunks
5. agent forwards partial and final transcript events back to the gateway
6. gateway normalizes them into the shared `asr.result` event model

## Transcript Revision Model

SpeechMesh does not expose ASR output as append-only tokens.

Instead, each `asr.result` event is a revisioned snapshot with:

- `revision` for monotonic ordering inside a segment
- `text` as the current best full text for that segment
- `delta` as an optimization hint
- `is_final` and `speech_final` for completion state

The important rule for SDKs and UI clients is simple:

- render `text`
- treat `delta` as optional optimization only

This handles common ASR behavior where earlier words can be revised after later context arrives.

## Extension Points

### Providers

Providers belong inside the capability domain they implement.

Examples:

- `asr/providers/apple`
- `asr/providers/whisper`
- `tts/providers/apple`
- `tts/providers/openai`

### Transports

The protocol model is transport-neutral enough to support future front doors, such as HTTP or gRPC, without moving provider logic out of the capability layer.

### SDKs

The current first-party SDKs are Go and Rust. Other languages should follow the same streaming semantics and preserve the same event model.

## Security Boundaries

The current security boundaries are intentionally simple:

- client traffic should terminate over TLS at the ingress layer
- `/agent` registration can require a shared secret
- the macOS bridge process should stay on a trusted host
- `bridge_tcpd` should not be exposed directly to untrusted networks

Client-side OAuth or multi-tenant auth is not built into the gateway yet; that should be layered in front of `/ws` and `/agent` if needed.

## Current Constraints

- one active session per client WebSocket connection
- ASR is the primary shipped transport path today
- Apple Speech execution requires macOS and cannot be containerized into the Linux gateway image
- the gateway is intentionally stateless and does not persist audio or transcripts
