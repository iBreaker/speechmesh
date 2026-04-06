# Go SDK

Go client SDK for `SpeechMesh`.

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
- receive `tts.audio.delta` and `tts.audio.done` events
- commit, stop, and close the active session

## Key Types

- `Client`
- `ClientConfig`
- `StreamRequest`
- `TtsStreamRequest`
- `RecognitionOptions`
- `TtsSynthesisOptions`
- `Event`

## Example

```go
ctx := context.Background()
client, err := speechmesh.Dial(ctx, speechmesh.ClientConfig{
    URL: "wss://speechmesh.example.com/ws",
})
if err != nil {
    return err
}
defer client.Close()

_, started, err := client.StartASR(ctx, speechmesh.StreamRequest{
    Provider:    speechmesh.DefaultProviderSelector(),
    InputFormat: speechmesh.PCMS16LE(16000, 1),
    Options: speechmesh.RecognitionOptions{InterimResults: true},
})
if err != nil {
    return err
}
_ = started
```

TTS example:

```go
ctx := context.Background()
client, err := speechmesh.Dial(ctx, speechmesh.ClientConfig{
	URL: "wss://speechmesh.example.com/ws",
})
if err != nil {
	return err
}
defer client.Close()

_, _, err = client.StartTTS(ctx, speechmesh.TtsStreamRequest{
	Provider:  speechmesh.DefaultProviderSelector(),
	InputKind: speechmesh.SynthesisInputKindText,
	Options: speechmesh.TtsSynthesisOptions{Stream: true},
})
if err != nil {
	return err
}

if err := client.TtsAppendInput(ctx, "hello from speechmesh"); err != nil {
	return err
}
if err := client.Commit(ctx); err != nil {
	return err
}
```

## Validation

```bash
cd sdks/go && go test ./...
cd sdks/go && go run ./examples/stream_asr --help
```

The repository's Go end-to-end example lives in `sdks/go/examples/stream_asr`.
