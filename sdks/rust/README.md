# Rust SDK

Async Rust client SDK for `SpeechMesh`.

## Scope

Current features:

- connect to the WebSocket gateway
- perform the `hello` handshake automatically
- discover ASR and TTS providers
- list TTS voices
- start a streaming ASR session
- start a TTS session
- stream PCM audio chunks as binary frames
- append TTS input text incrementally
- receive revision-based `asr.result` events
- receive `tts.audio.delta` and `tts.audio.done` events through `ServerMessage`
- commit, stop, and close the active session

## Key Types

- `Client`
- `ClientConfig`
- `StreamRequest`
- `TtsStreamRequest`
- `RecognitionOptions`
- `SynthesisOptions`
- `ServerMessage`

## Example

```rust,no_run
use speechmesh_sdk::{
    AudioFormat, Client, ClientConfig, ProviderSelector, RecognitionOptions, ServerMessage,
    StreamRequest,
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut client = Client::connect(ClientConfig::new("wss://speechmesh.example.com/ws")).await?;
let _providers = client.discover_asr().await?;
let started = client.start_asr(StreamRequest {
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

while let ServerMessage::AsrResult { payload, .. } = client.recv().await? {
    if payload.is_final && payload.speech_final {
        println!("final transcript: {}", payload.text);
        break;
    }
}

println!("session: {}", started.session_id);
# Ok(())
# }
```

TTS example:

```rust,no_run
use speechmesh_sdk::{
    Client, ClientConfig, ProviderSelector, SynthesisInputKind, SynthesisOptions, TtsStreamRequest,
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut client = Client::connect(ClientConfig::new("wss://speechmesh.example.com/ws")).await?;
let _started = client.start_tts(TtsStreamRequest {
    provider: ProviderSelector::default(),
    input_kind: SynthesisInputKind::Text,
    output_format: None,
    options: SynthesisOptions {
        stream: true,
        ..SynthesisOptions::default()
    },
}).await?;
client.append_tts_input("hello from speechmesh").await?;
client.commit().await?;
# Ok(())
# }
```

## Validation

```bash
cargo test -p speechmesh-sdk
cargo run --manifest-path examples/ws-asr-e2e/Cargo.toml -- --help
```

The repository's Rust end-to-end example in `examples/ws-asr-e2e` is built on this SDK.
