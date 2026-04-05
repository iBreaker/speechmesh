# WebSocket Protocol

SpeechMesh v1 is a WebSocket-first streaming protocol.

## Endpoints

- `/ws` - client-facing speech session endpoint
- `/agent` - macOS agent registration and execution endpoint

This document focuses on `/ws`, which is the stable public contract for SDKs and remote devices.

## Transport Rules

- one WebSocket connection supports at most one active speech session
- JSON text frames carry control-plane messages
- binary frames carry raw audio bytes for the active ASR session
- message types are tagged through the top-level `type` field
- request/response exchanges use `request_id` when applicable
- session-scoped events use `session_id`

## Connection Lifecycle

A typical client connection looks like this:

1. client connects to `/ws`
2. client sends `hello`
3. server replies with `hello.ok`
4. client optionally sends `discover`
5. client sends `asr.start`
6. server replies with `session.started`
7. client streams binary audio
8. client sends `asr.commit`
9. server emits `asr.result` revisions
10. server emits `session.ended`

## Client Messages

### `hello`

Sent once after opening the connection.

```json
{
  "type": "hello",
  "payload": {
    "protocol_version": "v1",
    "client_name": "speechmesh-rust-sdk"
  }
}
```

### `discover`

Used to enumerate available providers for one or more capability domains.

```json
{
  "type": "discover",
  "request_id": "req_1",
  "payload": {
    "domains": ["asr"]
  }
}
```

### `asr.start`

Starts a streaming ASR session.

```json
{
  "type": "asr.start",
  "request_id": "req_2",
  "payload": {
    "provider": {
      "mode": "auto",
      "required_capabilities": ["streaming-input"],
      "preferred_capabilities": ["on-device"]
    },
    "input_format": {
      "encoding": "pcm_s16le",
      "sample_rate_hz": 16000,
      "channels": 1
    },
    "options": {
      "language": "en-US",
      "hints": ["speechmesh"],
      "interim_results": true,
      "timestamps": false,
      "punctuation": true,
      "prefer_on_device": false
    }
  }
}
```

### Binary audio frames

After `session.started`, the client sends raw PCM bytes as binary WebSocket frames. The bytes must match the format negotiated in `input_format`.

### `asr.commit`

Signals that the client has finished sending audio for the active ASR session.

```json
{
  "type": "asr.commit",
  "session_id": "sess_123",
  "payload": {}
}
```

### `session.stop`

Stops the active session early.

```json
{
  "type": "session.stop",
  "session_id": "sess_123",
  "payload": {}
}
```

### `ping`

Optional liveness probe.

```json
{
  "type": "ping",
  "request_id": "req_ping",
  "payload": {}
}
```

## Server Messages

### `hello.ok`

```json
{
  "type": "hello.ok",
  "payload": {
    "protocol_version": "v1",
    "server_name": "speechmesh-gateway",
    "one_session_per_connection": true
  }
}
```

### `discover.result`

```json
{
  "type": "discover.result",
  "request_id": "req_1",
  "payload": {
    "providers": [
      {
        "id": "apple.asr",
        "name": "Apple ASR Agent",
        "domain": "asr",
        "runtime": "remote_gateway",
        "capabilities": [
          {"key": "streaming-input", "enabled": true},
          {"key": "on-device", "enabled": true}
        ]
      }
    ]
  }
}
```

### `session.started`

```json
{
  "type": "session.started",
  "request_id": "req_2",
  "session_id": "sess_123",
  "payload": {
    "domain": "asr",
    "provider_id": "apple.asr",
    "accepted_input_format": {
      "encoding": "pcm_s16le",
      "sample_rate_hz": 16000,
      "channels": 1
    }
  }
}
```

### `asr.result`

`asr.result` is a revision-based snapshot.

```json
{
  "type": "asr.result",
  "session_id": "sess_123",
  "sequence": 7,
  "payload": {
    "segment_id": 0,
    "revision": 7,
    "text": "Hello from speech mesh this is a streaming test",
    "delta": " test",
    "is_final": false,
    "speech_final": false,
    "begin_time_ms": null,
    "end_time_ms": null,
    "words": []
  }
}
```

A final result looks like:

```json
{
  "type": "asr.result",
  "session_id": "sess_123",
  "sequence": 8,
  "payload": {
    "segment_id": 0,
    "revision": 8,
    "text": "Hello from speech mesh this is an end to end streaming test",
    "delta": "Hello from speech mesh this is an end to end streaming test",
    "is_final": true,
    "speech_final": true,
    "begin_time_ms": null,
    "end_time_ms": null,
    "words": []
  }
}
```

## Delta Semantics

ASR decoders often revise earlier words after later context arrives.

SpeechMesh therefore applies these rules:

- `payload.text` is always the authoritative full text for the current revision
- `payload.delta` is best-effort only
- when the new text is a simple suffix append, `delta` contains just that suffix
- when the new text revises earlier content, `delta` can fall back to the full current text

Clients must render `payload.text` and never assume `delta` is append-only.

## Session Completion

The server ends an ASR session with `session.ended`.

```json
{
  "type": "session.ended",
  "session_id": "sess_123",
  "payload": {
    "reason": null
  }
}
```

Stop reading only after you have processed the terminal `asr.result` and the session has ended.

## Error Model

Errors use the shared `error` envelope.

```json
{
  "type": "error",
  "request_id": "req_2",
  "session_id": null,
  "payload": {
    "error": {
      "code": "unsupported",
      "message": "one active session per connection",
      "retryable": false,
      "details": null
    }
  }
}
```

Common error cases:

- malformed JSON control frames
- binary audio before `asr.start`
- starting a second session on the same connection
- unavailable provider or disconnected agent

## Public Contract Notes

- the WebSocket contract is provider-neutral by design
- provider-specific features should be expressed through capabilities and provider options
- SDKs should preserve the raw event model rather than hiding it too aggressively
