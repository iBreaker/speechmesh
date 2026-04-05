# ws-asr-e2e

WebSocket ASR end-to-end test client for SpeechMesh.

This example uses the first-party Rust SDK instead of speaking the wire protocol directly.

## Flow

1. connect to the SpeechMesh WebSocket endpoint
2. perform the `hello` handshake through the SDK
3. send `asr.start`
4. stream PCM audio as binary frames
5. send `asr.commit`
6. wait for `asr.result` revisions
7. stop on the final speech-complete result
8. optionally assert the final transcript contains expected text

## Usage

```bash
cargo run --manifest-path examples/ws-asr-e2e/Cargo.toml -- \
  --url ws://127.0.0.1:8080/ws \
  --wav /tmp/speechmesh_en.wav \
  --language en-US \
  --expected "speech mesh"
```

## Audio Requirements

The client expects input WAV as:

- mono
- 16 kHz
- PCM S16LE or convertible int/float WAV data

If your source audio is not already compatible, normalize it first:

```bash
scripts/prepare_asr_wav.sh --from ./input-any-format.wav /tmp/speechmesh_en.wav
```
