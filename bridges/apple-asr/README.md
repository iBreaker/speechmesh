# Apple ASR Bridge

`apple-asr-bridge` is a local macOS bridge process for streaming ASR with Apple `Speech` framework.

It reads newline-delimited JSON commands from `stdin` and writes newline-delimited JSON events to `stdout`.

The implementation keeps Speech calls on `@MainActor` and avoids blocking the main thread while waiting for authorization.

## Important Scope Note

This is an internal bridge protocol, not the public SpeechMesh client protocol.

Protocol surfaces are intentionally separated:

- public client API: WebSocket protocol on `/ws`
- internal agent protocol: WebSocket protocol on `/agent`
- internal bridge protocol: line-based JSON used by `apple-asr-bridge`

Internal bridge events such as `asr.partial` and `asr.final` are normalized by the agent and gateway into the public `asr.result` client event model.

## Build

```bash
cd bridges/apple-asr
swift build -c release
```

Binary path:

```bash
.build/release/apple-asr-bridge
```

## Command Protocol

Envelope:

```json
{
  "type": "asr.start",
  "request_id": "req-1",
  "session_id": "sess-1",
  "payload": {}
}
```

Supported `type` values:

- `hello`
- `auth.request`
- `asr.start`
- `asr.audio`
- `asr.commit`
- `asr.stop`
- `ping`
- `shutdown`

### `asr.start`

```json
{
  "type": "asr.start",
  "request_id": "req-start",
  "session_id": "sess-1",
  "payload": {
    "locale": "en-US",
    "should_report_partials": true,
    "requires_on_device": false,
    "input_format": {
      "encoding": "pcm_s16le",
      "sample_rate_hz": 16000,
      "channels": 1
    }
  }
}
```

### `asr.audio`

```json
{
  "type": "asr.audio",
  "request_id": "req-audio-1",
  "session_id": "sess-1",
  "payload": {
    "data_base64": "BASE64_PCM_CHUNK"
  }
}
```

### `asr.commit`

```json
{
  "type": "asr.commit",
  "request_id": "req-commit",
  "session_id": "sess-1",
  "payload": {}
}
```

## Events

Bridge emits:

- `bridge.ready`
- `hello.ok`
- `auth.result`
- `asr.started`
- `asr.partial`
- `asr.final`
- `asr.ended`
- `asr.committed`
- `pong`
- `shutdown.ok`
- `error`

## Runtime Caveats

- one active ASR session per `session_id`; multiple concurrent sessions are supported by session map, but each session must use matching `session_id`
- input audio must be base64-encoded `pcm_s16le`
- v1 validates and supports mono chunks and sample rate from `input_format`
- Speech recognition permission is required; call `auth.request` before `asr.start`
- the host process launching this bridge must have Speech permission on macOS
