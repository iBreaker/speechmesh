# SpeechMesh Swift SDK

`SpeechMeshSwift` is an iOS-ready Swift package for the public SpeechMesh `/ws` protocol.

It is designed to align with the repository's existing Go and Rust SDKs while adding Apple-platform helpers for:

- ASR provider discovery
- TTS provider discovery and voice listing
- streaming ASR sessions over WebSocket
- streaming TTS sessions over WebSocket
- session commit / stop lifecycle
- iOS microphone capture to SpeechMesh PCM S16LE input
- collecting TTS audio and playing it locally with `AVAudioPlayer`

## Package

```swift
.package(path: "../speechmesh/sdks/swift")
```

Product:

```swift
.product(name: "SpeechMeshSwift", package: "SpeechMeshSwift")
```

## Minimum platforms

- iOS 17+
- macOS 14+

## Usage

```swift
import SpeechMeshSwift

let client = SpeechMeshClient(
    config: SpeechMeshClientConfig(
        url: URL(string: "wss://speechmesh.example.com/ws")!,
        clientName: "ios-demo"
    )
)

try await client.connect()
let providers = try await client.discoverASR()
print(providers.providers)
```

### Streaming ASR from microphone

```swift
let request = AsrStreamRequest(
    provider: .init(),
    inputFormat: .pcmS16LE(sampleRateHz: 16_000, channels: 1),
    options: RecognitionOptions(
        language: "zh-CN",
        interimResults: true,
        punctuation: true,
        preferOnDevice: false
    )
)

let _ = try await client.startASR(request)
let microphone = SpeechMeshMicrophoneStreamer()
try await microphone.start { chunk in
    Task {
        try? await client.sendAudio(chunk)
    }
}

try await client.commit()
while true {
    let event = try await client.receive()
    switch event {
    case .asrResult(_, _, let payload):
        print(payload.text)
        if payload.isFinal && payload.speechFinal {
            break
        }
    case .sessionEnded:
        break
    default:
        continue
    }
}
```

### Streaming TTS and local playback

```swift
let voices = try await client.listTTSVoices(.init(provider: .provider("melo.tts")))
print(voices.voices)

let _ = try await client.startTTS(
    TtsStreamRequest(
        provider: .provider("melo.tts"),
        inputKind: .text,
        options: SynthesisOptions(stream: true)
    )
)

try await client.appendTTSInput("你好，我是 iOS 版 SpeechMesh。")
try await client.commit()

var collector = SpeechMeshTTSCollector()
while true {
    let event = try await client.receive()
    switch event {
    case .ttsAudioDelta(_, _, let payload):
        try collector.append(payload)
    case .ttsAudioDone:
        let player = SpeechMeshAudioPlayer()
        try player.play(collector.collectedAudio())
    case .sessionEnded:
        break
    default:
        continue
    }
}
```

## iOS integration notes

Your app should include the standard microphone permission key before using `SpeechMeshMicrophoneStreamer`:

- `NSMicrophoneUsageDescription`

If you also do local Apple Speech recognition separately in your app, add:

- `NSSpeechRecognitionUsageDescription`

This package itself talks to the SpeechMesh gateway and does not embed the repository's macOS-only `apple-asr-bridge`.
