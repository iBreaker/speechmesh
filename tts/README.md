# TTS

`tts` is the text-to-speech capability domain.

## Responsibilities

- synthesis request contracts
- voice and output descriptors
- provider-facing option types
- transport-backed TTS runtime behavior

The transport-backed implementation path now exists through the generic WebSocket lifecycle in `speechmeshd`, with MeloTTS as the first concrete provider.

For current provider research, see `docs/tts-landscape.md`.
