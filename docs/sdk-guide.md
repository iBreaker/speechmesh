# SDK Guide

SpeechMesh ships first-party client SDKs so remote devices can use a single gateway endpoint without embedding platform-specific speech SDKs locally.

## Supported SDKs

| Language | Path | Status | Notes |
| --- | --- | --- | --- |
| Rust | `sdks/rust` | usable | async WebSocket client built on `tokio-tungstenite` |
| Go | `sdks/go` | usable | WebSocket client built on `github.com/coder/websocket` |

Both SDKs target the same public WebSocket contract and the same split deployment topology:

- clients run anywhere
- the gateway runs centrally
- Apple Speech still executes on a macOS host through the SpeechMesh agent path

## Common Client Lifecycle

Every SDK follows the same sequence:

1. connect to the WebSocket gateway
2. perform the `hello` handshake automatically
3. discover providers if needed
4. start one ASR session
5. stream binary audio chunks
6. commit or stop the session
7. consume `asr.result` revisions until the final result arrives
8. close the connection or start a new one

## Event Handling Rules

The most important rules for client implementations are:

- one active session per connection
- `payload.text` is the authoritative current transcript
- `payload.delta` is only an optimization hint
- final completion is `is_final=true` and `speech_final=true`
- clients should keep reading until they also observe `session.ended`

## Rust SDK

Path: `sdks/rust`

Example:

```rust,no_run
use speechmesh_sdk::{
    AudioFormat, Client, ClientConfig, ProviderSelector, RecognitionOptions, StreamRequest,
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut client = Client::connect(ClientConfig::new("wss://speechmesh.example.com/ws")).await?;

let _providers = client.discover_asr().await?;
let _started = client.start_asr(StreamRequest {
    provider: ProviderSelector::default(),
    input_format: AudioFormat::pcm_s16le(16_000, 1),
    options: RecognitionOptions {
        language: Some("en-US".to_string()),
        interim_results: true,
        punctuation: true,
        ..RecognitionOptions::default()
    },
}).await?;

client.send_audio(&[0_u8; 3200]).await?;
client.commit().await?;
# Ok(())
# }
```

Live example client:

- `examples/ws-asr-e2e`

## Go SDK

Path: `sdks/go`

Example:

```go
ctx := context.Background()
client, err := speechmesh.Dial(ctx, speechmesh.ClientConfig{
    URL: "wss://speechmesh.example.com/ws",
})
if err != nil {
    return err
}
defer client.Close()

providers, err := client.DiscoverASR(ctx)
if err != nil {
    return err
}
_ = providers
```

Live example client:

- `sdks/go/examples/stream_asr`

## Audio Format Expectations

The current examples assume:

- mono audio
- 16 kHz sample rate
- PCM S16LE payloads

That matches the helper scripts and the current Apple ASR deployment path.

## Reconnection Guidance

SpeechMesh does not currently support resuming an in-flight ASR session after transport loss.

If the connection drops:

- open a new WebSocket connection
- start a new ASR session
- replay audio if your application requires it

## Choosing Providers

The shared provider selector supports:

- automatic selection
- direct provider pinning
- required capability flags
- preferred capability flags

Use provider capabilities to express runtime intent, such as `streaming-input` or `on-device`, instead of hard-coding transport-specific logic into the client.
