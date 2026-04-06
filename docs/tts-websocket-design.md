# TTS WebSocket Design

SpeechMesh TTS over WebSocket follows the common shape used by mature realtime TTS APIs:

- open one WebSocket session
- negotiate synthesis settings once
- append text incrementally
- commit when the current buffered text is ready
- stream audio chunks back as ordered events
- close with an explicit terminal event

## Vendor Patterns We Borrowed

The current design was informed by official realtime TTS APIs from:

- Alibaba Cloud Qwen realtime TTS
- MiniMax speech WebSocket TTS
- ElevenLabs WebSocket TTS
- Volcengine realtime TTS

Across those APIs, the recurring patterns are very consistent:

1. a session-level start or update message configures voice, output format, and generation options
2. text can arrive incrementally rather than needing to be sent as one giant payload
3. audio is emitted as a stream of ordered deltas or chunks
4. there is always an explicit flush, commit, or end-of-input signal
5. completion and error events are first-class messages, not implicit socket shutdown

## SpeechMesh Choices

SpeechMesh keeps those ideas but normalizes them into the existing capability-first protocol.

### Client Messages

- `tts.voices`
- `tts.start`
- `tts.input.append`
- `tts.commit`
- `session.stop`

### Server Messages

- `tts.voices.result`
- `session.started`
- `tts.audio.delta`
- `tts.audio.done`
- `session.ended`
- `error`

## Why `append + commit`

This is the most future-proof contract shape because it works for both categories of backend:

- buffered engines such as MeloTTS, which need the full text before synthesis
- true streaming engines, which can begin synthesis before the client has finished sending all text

For buffered engines, `tts.input.append` simply fills an internal buffer and `tts.commit` starts synthesis.

For streaming-native engines, the same two messages can later map to:

- low-latency incremental text feeding
- a final flush to end the current utterance

## Why `tts.audio.delta`

SpeechMesh currently sends audio chunks as JSON events with Base64 payloads:

- it matches the most common vendor WebSocket style
- it keeps control and media on one logical event stream
- it makes debugging and protocol inspection easy

This does add Base64 overhead. A future protocol revision can add optional binary-frame TTS output without breaking the higher-level session lifecycle.

## Why `tts.audio.done`

Many vendor APIs have a distinct synthesis completion event separate from socket close.

SpeechMesh keeps that pattern:

- `tts.audio.done` means audio emission is complete for this request
- `session.ended` means the session itself is over

That makes clients easier to write and easier to test.

## Current MeloTTS Mapping

The first SpeechMesh TTS provider is `melo.tts`:

- `tts.start` configures the session and validates voice/language expectations
- `tts.input.append` buffers text
- `tts.commit` calls the local MeloTTS HTTP server
- the returned WAV is chunked into ordered `tts.audio.delta` events
- the session closes with `tts.audio.done` and `session.ended`

This gives us a stable generic gateway API now, while still leaving room for more advanced streaming engines later.
