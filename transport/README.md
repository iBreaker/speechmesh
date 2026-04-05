# Transport

`transport` defines the shared wire-level contracts used to expose SpeechMesh outside a single process.

## Responsibilities

- message envelopes
- request and response payloads
- event payloads
- transport-neutral error behavior

## Guardrails

- keep transport generic across ASR and TTS
- avoid provider-specific fields in shared message types
- preserve streaming semantics needed by SDKs and runtimes
