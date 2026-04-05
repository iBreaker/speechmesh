# Core

`core` holds SpeechMesh runtime concepts shared across ASR, TTS, transports, SDKs, and gateway implementations.

## Responsibilities

- provider descriptors and runtime modes
- capability declarations
- session identifiers
- audio format descriptors
- shared error structures

These types should stay small, stable, and provider-neutral.
