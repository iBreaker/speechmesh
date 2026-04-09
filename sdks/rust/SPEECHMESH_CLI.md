# speechmesh-cli

`speechmesh-cli` is the unified SpeechMesh client binary.

It talks to a SpeechMesh gateway over the stable `/ws` WebSocket endpoint and is intended to be usable by both humans and automation/agents without reading the source code.

## Core commands

```bash
speechmesh-cli discover providers --json
speechmesh-cli tts voices --provider minimax.tts --json
speechmesh-cli tts play --provider minimax.tts --text "Hello from SpeechMesh"
speechmesh-cli tts stream --provider minimax.tts --text "Hello" > out.mp3
speechmesh-cli asr transcribe --provider mock.asr --stdin < audio.pcm
speechmesh-cli say --device mac03 --text "Route this to a remote speaker"
speechmesh-cli say --text "Use the configured default target device"
```

## Important behavior

- `tts play` streams audio to the local speaker and requires `ffplay` on `PATH`.
- `tts stream` writes raw audio bytes to `stdout`; progress and status messages go to `stderr`.
- `asr transcribe` reads bytes from `--file` or `--stdin` and prints the final text to `stdout`.
- All commands accept `--url` to point at a different SpeechMesh `/ws` endpoint.
- Use `--json` for one-shot structured output and `--jsonl` for streaming machine-readable events.
- `say` sends TTS to the gateway and then tells a remote `speechmesh-agent` to play it on that machine's current OS default output device.
- Most runtime options can be omitted when matching values are set under `profiles.<name>.defaults` in `~/.speechmesh/config.yml`.

## Client config defaults

Default config path: `~/.speechmesh/config.yml`

```yaml
active_profile: default
profiles:
  default:
    gateway:
      ws_url: wss://speechmesh.svc.lan.breaker.host/ws
      control_url: wss://speechmesh.svc.lan.breaker.host/control
      agent_url: wss://speechmesh.svc.lan.breaker.host/agent
    client_name: speechmesh-cli
    shared_secret: speechmesh-agent-20260405-6c0e7f4b
    defaults:
      device: mac03
      provider: minimax.tts
      voice: female-shaonv
      language: zh-CN
      volume: 1.2
      format: mp3
      sample_rate: 32000
      channels: 1
      rate: 1.0
      pitch: 1.0
      require: low-latency,streaming
      prefer: natural
      control_url: wss://speechmesh.svc.lan.breaker.host/control
      chunk_size_bytes: 16384
      asr_encoding: wav
      asr_sample_rate: 16000
      asr_channels: 1
      asr_language: zh-CN
      asr_interim: false
      asr_punctuation: true
      asr_timestamps: false
      asr_hints: speechmesh,codex
```

- `active_profile` selects the profile used by default.
- `profiles.<name>.gateway.*` provides the gateway endpoints for `speechmesh-cli` and `speechmesh-agent`.
- `profiles.<name>.shared_secret` is the agent handshake secret.
- `profiles.<name>.defaults.*` provides per-client fallbacks for `say`, `tts`, and `asr`. Any flag (`--device`, `--provider`, `--volume`, `--encoding`, etc.) passed on the command line overrides the defaults; if a flag is omitted and a matching `defaults` entry exists the CLI uses it automatically, otherwise the provider or gateway picks a sensible value.

### Default coverage

- `say`:
  - `device`, `chunk_size_bytes`, `control_url`
  - TTS-specific parameters inherited from `tts play/stream`: `provider`, `voice`, `language`, `rate`, `pitch`, `volume`, `format`, `sample_rate`, `channels`
- `tts`:
  - `provider`, `voice`, `language`, `rate`, `pitch`, `volume`, `format`, `sample_rate`, `channels`, `require`, `prefer`
- `asr`:
  - `provider`, `language`, `encoding` (`asr_encoding`), `sample_rate` (`asr_sample_rate`), `channels` (`asr_channels`), `interim_results`, `punctuation`, `timestamps`, `hints`

The CLI aggregates these `defaults` once per run, so you can keep your commands concise while still overriding any value by passing an explicit flag.

## Transport assumptions

- The gateway must be reachable at `--url`.
- TTS/ASR providers must already be installed on the target gateway.
- `tts play` depends on a local audio player; the current implementation uses `ffplay`.
- `speechmesh-cli doctor` is available today and runs a `/ws` health check plus optional `/control` play test.
- `speechmesh-cli devices` and `speechmesh-cli agent status` are planned. Once the `/control` endpoints introduced in `speechmeshd` land, they will expose registered agents/devices and recent task status; the CLI will respect `profiles.<name>.defaults.device` when targeting a specific machine.
