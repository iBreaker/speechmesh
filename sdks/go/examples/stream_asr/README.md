# stream_asr

Go SDK example for streaming ASR against the SpeechMesh WebSocket gateway.

## Usage

```bash
go run ./examples/stream_asr \
  --url wss://speechmesh.example.com/ws \
  --wav /tmp/speechmesh_en.wav \
  --expected "speech mesh"
```
