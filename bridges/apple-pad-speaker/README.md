# iPad SpeechMesh Speaker Binary

This is a dedicated iPad/iOS playback helper for SpeechMesh `pad` devices.

It reads audio bytes from `stdin` and plays them through
`AVAudioSession` + `AVAudioPlayer`, then exits after playback ends.

## Build

```bash
cd bridges/apple-pad-speaker
swift build -c release
```

Release binary path (Swift build layout):

- `.build/release/speechmesh-pad-speaker`

## Runtime contract

- Input: audio stream from `speechmesh agent run` pipe (`stdin`), supports MP3/WAV/FLAC depending on runtime codec availability.
- Exit code: `0` when playback completes, non-zero when a fatal playback error happens.
- Logging:
  - Writes progress and failures to `stderr`.

## Use with speechmesh agent

```bash
SPEECHMESH_PAD_PLAYER_CMD="/path/to/speechmesh-pad-speaker"
speechmesh agent run --agent-id pad-agent --device-id pad ...
```

If you also need a fallback path for non-iPad devices, keep
`SPEECHMESH_PLAYBACK_CMD` configured as before.
