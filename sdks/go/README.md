# Go SDK

Go client SDK for `SpeechMesh`.

## Scope

Current features:

- connect to the WebSocket gateway
- perform the `hello` handshake automatically
- discover providers
- start a streaming ASR session
- stream PCM audio chunks as binary frames
- receive revision-based `asr.result` events
- commit, stop, and close the active session

## Key Types

- `Client`
- `ClientConfig`
- `StreamRequest`
- `RecognitionOptions`
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

## Validation

```bash
cd sdks/go && go test ./...
cd sdks/go && go run ./examples/stream_asr --help
```

The repository's Go end-to-end example lives in `sdks/go/examples/stream_asr`.
